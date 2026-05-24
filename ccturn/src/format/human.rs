use chrono::DateTime;

use crate::extract::errors::{ErrorCategory, ErrorRecord};
use crate::extract::interventions::InterventionKind;
use crate::report::SessionReport;

const TOOLS_DISPLAY_LIMIT: usize = 10;

// § Timestamp formatting policy: human output parses each source timestamp and
// reformats to second precision with a `Z` suffix. A parse failure falls back
// to the raw string (only reachable on malformed input, never in well-formed
// sessions).
fn fmt_timestamp(raw: &str) -> String {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        Err(_) => raw.to_string(),
    }
}

fn fmt_time(raw: &str) -> String {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.format("%H:%M:%SZ").to_string(),
        Err(_) => raw.to_string(),
    }
}

fn fmt_duration(total_secs: i64) -> String {
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn fmt_span(started: &str, ended: &str) -> String {
    let start = DateTime::parse_from_rfc3339(started);
    let end = DateTime::parse_from_rfc3339(ended);
    match (start, end) {
        (Ok(s), Ok(e)) => format!(
            "{} → {}  ({})",
            fmt_timestamp(started),
            fmt_timestamp(ended),
            fmt_duration((e - s).num_seconds().max(0))
        ),
        _ => format!("{} → {}", fmt_timestamp(started), fmt_timestamp(ended)),
    }
}

fn pluralize(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

fn category_name(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::UserRejection => "UserRejection",
        ErrorCategory::PermissionDenied => "PermissionDenied",
        ErrorCategory::HookBlock => "HookBlock",
        ErrorCategory::Technical => "Technical",
    }
}

pub fn format_human(report: &SessionReport) -> String {
    let mut out = String::new();

    // Header block.
    out.push_str(&format!("Session  {}\n", report.session_id));
    out.push_str(&format!("Project  {}\n", report.project_cwd));
    out.push_str(&format!(
        "Span     {}\n",
        fmt_span(&report.started_at, &report.ended_at)
    ));
    out.push_str(&format!("Records  {} lines\n", report.record_count));
    out.push('\n');

    // Skills.
    out.push_str(&format!("== Skills ({}) ==\n", report.skills.len()));
    for skill in &report.skills {
        out.push_str(&format!(
            "  {}  1 invocation, {} inner errors, window {} tool uses\n",
            skill.skill_name, skill.inner_errors, skill.inner_tool_uses
        ));
    }
    out.push('\n');

    // Subagents.
    out.push_str(&format!("== Subagents ({}) ==\n", report.subagents.len()));
    for sub in &report.subagents {
        let secs = sub.total_duration_ms as f64 / 1000.0;
        out.push_str(&format!(
            "  {}  {}  status={}  {:.1}s  {} tok   bash={} read={} search={} edit={}\n",
            sub.agent_type,
            sub.agent_id,
            sub.status,
            secs,
            sub.total_tokens,
            sub.tool_stats.bash_count,
            sub.tool_stats.read_count,
            sub.tool_stats.search_count,
            sub.tool_stats.edit_file_count
        ));
        out.push_str(&format!("    description: {}\n", sub.description));
        out.push_str(&format!("    log: {}\n", sub.log_path));
    }
    out.push('\n');

    // Tools — header is the literal "(top 10 by use)" label, not a count; at
    // most 10 rows. `Skill` / `Task` rows carry a cross-reference marker.
    out.push_str("== Tools (top 10 by use) ==\n");
    for tool in report.tools.iter().take(TOOLS_DISPLAY_LIMIT) {
        let marker = match tool.tool_name.as_str() {
            "Skill" => "   [see Skills]",
            "Task" => "   [see Subagents]",
            _ => "",
        };
        out.push_str(&format!(
            "  {}  {} {}, {} {}{}\n",
            tool.tool_name,
            tool.invocation_count,
            pluralize(tool.invocation_count, "invocation"),
            tool.error_count,
            pluralize(tool.error_count, "error"),
            marker
        ));
    }
    out.push('\n');

    // Errors — grouped into the four categories, each with its own count.
    out.push_str(&format!("== Errors ({}) ==\n", report.errors.len()));
    for category in [
        ErrorCategory::UserRejection,
        ErrorCategory::PermissionDenied,
        ErrorCategory::HookBlock,
        ErrorCategory::Technical,
    ] {
        let group: Vec<&ErrorRecord> = report
            .errors
            .iter()
            .filter(|e| e.category == category)
            .collect();
        out.push_str(&format!(
            "  {} ({}):\n",
            category_name(category),
            group.len()
        ));
        for err in group {
            let input_part = err
                .input_excerpt
                .as_deref()
                .map(|s| format!("  \"{s}\""))
                .unwrap_or_default();
            // UserRejection's tool_result content is the same boilerplate
            // string on every row ("The user doesn't want to proceed..."), so
            // the only differentiator a reader actually cares about is the
            // input the agent was trying to run. Suppress the excerpt only
            // when we have an input to show — if the originating tool_use is
            // missing or its input shape is unrecognised, fall back to the
            // excerpt so the row is not just `tool  toolu_id` with no signal.
            if err.category == ErrorCategory::UserRejection && err.input_excerpt.is_some() {
                out.push_str(&format!(
                    "    {}  {}{}\n",
                    err.tool_name, err.tool_use_id, input_part
                ));
            } else {
                out.push_str(&format!(
                    "    {}  {}{}  \"{}\"\n",
                    err.tool_name, err.tool_use_id, input_part, err.excerpt
                ));
            }
        }
    }
    out.push('\n');

    // Interventions.
    out.push_str(&format!(
        "== Interventions ({}) ==\n",
        report.interventions.len()
    ));
    for iv in &report.interventions {
        match iv.kind {
            InterventionKind::Error => {
                let tool = iv.tool_name.as_deref().unwrap_or("<unknown>");
                let input_part = iv
                    .input_excerpt
                    .as_deref()
                    .map(|s| format!(" \"{s}\""))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  [error]  {}{}  at {}\n",
                    tool,
                    input_part,
                    fmt_time(&iv.timestamp)
                ));
            }
            InterventionKind::UserMidStream => {
                out.push_str(&format!(
                    "  [user]   mid-stream user message at {}\n",
                    fmt_time(&iv.timestamp)
                ));
                out.push_str(&format!("    \"{}\"\n", iv.excerpt));
            }
        }
    }
    out.push('\n');

    out
}

// Phase A test specification for the Human formatter (design doc § Implementation
// :: Step 10 and § Human Output). The Programmer adds the production code ABOVE
// this `#[cfg(test)]` block in Phase B:
//   - `pub fn format_human(report: &SessionReport) -> String` — renders the
//     section layout from § Human Output: a header block (Session / Project /
//     Span / Records) followed by the five sections Skills, Subagents, Tools,
//     Errors, Interventions. Every section header is emitted even when its
//     list is empty (`== Skills (0) ==`) so downstream `grep` stays stable.
//
// Per § Timestamp formatting policy, human output parses each source timestamp
// and reformats it to second precision with a `Z` suffix
// (`YYYY-MM-DDTHH:MM:SSZ`) — the verbatim microsecond `+00:00` form is JSON-only.
//
// Per § Tools-section composition, the `Skill` and `Task` rows in the Tools
// section carry trailing `[see Skills]` / `[see Subagents]` markers.

#[cfg(test)]
mod tests {
    use super::format_human;
    use crate::locator::{CwdSource, ResolvedSession};
    use crate::report::{SessionReport, build_report};
    use std::path::PathBuf;

    fn report_from_fixture(jsonl_name: &str) -> SessionReport {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let resolved = ResolvedSession {
            jsonl_path: fixtures.join(jsonl_name),
            subagents_dir: fixtures.join("full-session-subagents"),
            project_cwd_encoded: "-tmp-test-project".to_string(),
            project_cwd: PathBuf::from("/tmp/test-project"),
            cwd_source: CwdSource::FirstRecord,
        };
        build_report(&resolved)
            .unwrap_or_else(|e| panic!("build_report failed for {jsonl_name}: {e}"))
    }

    fn line_containing<'a>(output: &'a str, needle: &str) -> &'a str {
        output
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line containing {needle:?} in human output:\n{output}"))
    }

    // ---------------------------------------------------------------------
    // All five section headers appear for a populated report, each with the
    // parenthesised count. (The Tools header is asserted leniently — the
    // design doc § Human Output illustrates it as `== Tools (top 10 by use)
    // ==` rather than a bare count, so this test only pins that the header
    // line exists and is bracketed by `== Tools` ... `==`.)
    // ---------------------------------------------------------------------

    #[test]
    fn all_section_headers_appear_for_a_populated_report() {
        let report = report_from_fixture("full-session.jsonl");
        let output = format_human(&report);

        assert!(
            output.contains("== Skills (1) =="),
            "Skills header with count missing from:\n{output}"
        );
        assert!(
            output.contains("== Subagents (1) =="),
            "Subagents header with count missing from:\n{output}"
        );
        let tools_header = line_containing(&output, "== Tools");
        assert!(
            tools_header.contains("=="),
            "Tools header line must be bracketed by `==`: {tools_header:?}"
        );
        assert!(
            output.contains("== Errors (4) =="),
            "Errors header with count missing from:\n{output}"
        );
        assert!(
            output.contains("== Interventions (4) =="),
            "Interventions header with count missing from:\n{output}"
        );
    }

    // ---------------------------------------------------------------------
    // Every section header still appears when its list is empty — the
    // `empty-sections.jsonl` fixture has only plain text turns, so the
    // report's five arrays are all empty. The headers must still render.
    // ---------------------------------------------------------------------

    #[test]
    fn all_section_headers_appear_even_when_every_section_is_empty() {
        let report = report_from_fixture("empty-sections.jsonl");
        let output = format_human(&report);

        assert!(
            output.contains("== Skills (0) =="),
            "empty Skills header missing from:\n{output}"
        );
        assert!(
            output.contains("== Subagents (0) =="),
            "empty Subagents header missing from:\n{output}"
        );
        // Tools header asserted leniently (see note on the populated test).
        let tools_header = line_containing(&output, "== Tools");
        assert!(
            tools_header.contains("=="),
            "empty Tools header line must still appear: {tools_header:?}"
        );
        assert!(
            output.contains("== Errors (0) =="),
            "empty Errors header missing from:\n{output}"
        );
        assert!(
            output.contains("== Interventions (0) =="),
            "empty Interventions header missing from:\n{output}"
        );
    }

    // ---------------------------------------------------------------------
    // Timestamp formatting policy — human output reformats each timestamp to
    // second precision with a `Z` suffix. The verbatim microsecond `+00:00`
    // form (kept only in JSON output) must NOT leak into human output.
    // ---------------------------------------------------------------------

    #[test]
    fn timestamps_are_reformatted_to_second_precision_with_z_suffix() {
        let report = report_from_fixture("full-session.jsonl");
        let output = format_human(&report);

        // started_at "2026-05-19T09:00:00.000000+00:00" -> "2026-05-19T09:00:00Z"
        assert!(
            output.contains("2026-05-19T09:00:00Z"),
            "reformatted start timestamp missing from:\n{output}"
        );
        // ended_at "2026-05-19T09:00:18.000000+00:00" -> "2026-05-19T09:00:18Z"
        assert!(
            output.contains("2026-05-19T09:00:18Z"),
            "reformatted end timestamp missing from:\n{output}"
        );
        assert!(
            !output.contains(".000000"),
            "human output must not carry verbatim microsecond precision"
        );
        assert!(
            !output.contains("+00:00"),
            "human output must not carry the verbatim `+00:00` offset; \
             timestamps are reformatted to a `Z` suffix"
        );
    }

    // ---------------------------------------------------------------------
    // Tools-section composition — `Skill` and `Task` are themselves
    // `tool_use` blocks, so they appear as Tools rows; each must carry a
    // trailing `[see Skills]` / `[see Subagents]` cross-reference marker.
    // ---------------------------------------------------------------------

    #[test]
    fn tools_section_marks_skill_and_task_rows_with_cross_reference() {
        let report = report_from_fixture("full-session.jsonl");
        let output = format_human(&report);

        let skill_row = line_containing(&output, "[see Skills]");
        assert!(
            skill_row.contains("Skill"),
            "the `[see Skills]` marker must sit on the Skill tool row: {skill_row:?}"
        );

        let task_row = line_containing(&output, "[see Subagents]");
        assert!(
            task_row.contains("Task"),
            "the `[see Subagents]` marker must sit on the Task tool row: {task_row:?}"
        );
    }
}
