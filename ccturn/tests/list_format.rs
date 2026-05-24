// Phase 2 Step 5 + 6 — Human and JSON formatters for `crates` / `tracks`.
//
// Step 5: `src/format/list_human.rs` with three entry points:
//   * `format_projects(&ProjectListing) -> String`
//   * `format_sessions_default(&SessionListing, subagents_dir_resolver) -> String`
//   * `format_sessions_oneline(&SessionListing) -> String`
//
// Step 6: `src/format/list_json.rs` with:
//   * `format_projects_json(&ProjectListing) -> String`
//   * `format_sessions_json(&SessionListing) -> String`
//
// The tests in this file exercise the formatters with hand-built
// `ProjectListing` / `SessionListing` values so they decouple from the
// enumeration modules. Tempfiles supply meta.json sidecars for the
// default-mode `Subagents` sub-block tests.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use tempfile::TempDir;

use ccturn::format::list_human::{
    format_projects, format_sessions_default, format_sessions_oneline,
};
use ccturn::format::list_json::{format_projects_json, format_sessions_json};
use ccturn::list::projects::{ProjectListing, ProjectRow};
use ccturn::list::sessions::{
    SessionListing, SessionRow, SessionStatus, SubagentSummary, ToolStats,
};
use ccturn::locator::CwdSource;

// ---- Timestamp helpers --------------------------------------------------

// Typed DateTime — used for ProjectRow.latest_session_at (Option<DateTime<Utc>>).
fn ts(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
        .single()
        .expect("test timestamp must be valid")
}

// Raw ISO 8601 string — used for SessionRow.started_at / ended_at
// (Option<String>; the human formatter is responsible for reformatting to
// second precision with the `Z` suffix per § tracks PROJECT > Date line).
fn ts_string(year: i32, month: u32, day: u32, hour: u32, min: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:00.000Z")
}

// ---- Builders -----------------------------------------------------------

fn project_row(
    encoded: &str,
    cwd: Option<&str>,
    cwd_source: CwdSource,
    session_count: usize,
    latest: Option<DateTime<Utc>>,
) -> ProjectRow {
    ProjectRow {
        encoded_cwd: encoded.to_string(),
        project_cwd: cwd.map(PathBuf::from),
        cwd_source,
        session_count,
        latest_session_at: latest,
    }
}

fn project_listing(log_root: &Path, projects: Vec<ProjectRow>) -> ProjectListing {
    ProjectListing {
        log_root: log_root.to_path_buf(),
        projects,
    }
}

fn empty_tool_stats() -> ToolStats {
    ToolStats {
        read_count: 0,
        search_count: 0,
        bash_count: 0,
        edit_file_count: 0,
        lines_added: 0,
        lines_removed: 0,
        other_tool_count: 0,
    }
}

fn task_stats(read: u64, search: u64, bash: u64, edit: u64) -> ToolStats {
    ToolStats {
        read_count: read,
        search_count: search,
        bash_count: bash,
        edit_file_count: edit,
        lines_added: 0,
        lines_removed: 0,
        other_tool_count: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn subagent(
    agent_id: &str,
    agent_type: &str,
    tool_use_id: &str,
    status: &str,
    duration: Option<u64>,
    tokens: Option<u64>,
    tool_use_count: Option<u64>,
    stats: Option<ToolStats>,
) -> SubagentSummary {
    SubagentSummary {
        agent_id: agent_id.to_string(),
        agent_type: agent_type.to_string(),
        tool_use_id: tool_use_id.to_string(),
        description: None,
        status: status.to_string(),
        total_duration_ms: duration,
        total_tokens: tokens,
        total_tool_use_count: tool_use_count,
        tool_stats: stats,
        log_path: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn session_row(
    session_id: &str,
    log_path: &Path,
    started_at: Option<String>,
    ended_at: Option<String>,
    status: SessionStatus,
    ai_title: Option<&str>,
    excerpt: Option<&str>,
    open_tool_uses: usize,
    last_tool_result_is_error: bool,
    subagents: Vec<SubagentSummary>,
) -> SessionRow {
    SessionRow {
        session_id: session_id.to_string(),
        log_path: log_path.to_path_buf(),
        started_at,
        ended_at,
        status,
        ai_title: ai_title.map(str::to_string),
        first_user_message_excerpt: excerpt.map(str::to_string),
        open_tool_uses,
        last_tool_result_is_error,
        subagents,
    }
}

#[allow(clippy::too_many_arguments)]
fn session_listing(
    log_root: &Path,
    encoded_cwd: &str,
    project_cwd: Option<&str>,
    cwd_source: CwdSource,
    session_count: usize,
    limit: usize,
    sessions: Vec<SessionRow>,
) -> SessionListing {
    SessionListing {
        log_root: log_root.to_path_buf(),
        encoded_cwd: encoded_cwd.to_string(),
        project_cwd: project_cwd.map(PathBuf::from),
        cwd_source,
        session_count,
        limit,
        sessions,
    }
}

// Convenience: build a barebones session with the given timestamp / status /
// title and no subagents and no excerpt.
fn fixture_session(
    session_id: &str,
    started: String,
    status: SessionStatus,
    ai_title: Option<&str>,
) -> SessionRow {
    let log_path = PathBuf::from(format!("/tmp/log/{session_id}.jsonl"));
    session_row(
        session_id,
        &log_path,
        Some(started.clone()),
        Some(started),
        status,
        ai_title,
        None,
        0,
        false,
        Vec::new(),
    )
}

// Build a `<tmp>/<session_id>/subagents/agent-<agent_id>.meta.json` sidecar.
fn write_meta_json(tmp: &Path, session_id: &str, agent_id: &str, description: &str) {
    let dir = tmp.join(session_id).join("subagents");
    fs::create_dir_all(&dir).expect("create subagents dir");
    let body = format!(
        r#"{{"description":{json}}}"#,
        json = serde_json::to_string(description).unwrap()
    );
    fs::write(dir.join(format!("agent-{agent_id}.meta.json")), body).expect("write meta.json");
}

// Default-mode resolver mapping session_id to <tmp>/<session_id>/subagents.
// Programmer's eventual closure signature may differ; the tests below
// assume `impl Fn(&str) -> PathBuf` because the design doc names the
// closure `subagents_dir_resolver` and that is the smallest signature
// that satisfies the spec.
fn make_resolver(tmp: PathBuf) -> impl Fn(&str) -> PathBuf {
    move |session_id: &str| tmp.join(session_id).join("subagents")
}

// =======================================================================
// Step 5: format_projects (human)
// =======================================================================

#[test]
fn format_projects_header_lists_log_root_and_count() {
    let log_root = PathBuf::from("/tmp/test-log-root");
    let listing = project_listing(
        &log_root,
        vec![
            project_row(
                "-tmp-a",
                Some("/tmp/a"),
                CwdSource::FirstRecord,
                3,
                Some(ts(2026, 5, 22, 18, 30)),
            ),
            project_row(
                "-tmp-b",
                Some("/tmp/b"),
                CwdSource::FirstRecord,
                1,
                Some(ts(2026, 5, 21, 10, 15)),
            ),
        ],
    );
    let out = format_projects(&listing);
    let first_line = out.lines().next().expect("header line must exist");
    assert!(
        first_line.contains("/tmp/test-log-root"),
        "header must name the resolved log root verbatim; got: {first_line}"
    );
    assert!(
        first_line.contains("(2 projects)"),
        "header must list the project count as `(N projects)`; got: {first_line}"
    );
}

#[test]
fn format_projects_empty_log_root_still_emits_header() {
    let log_root = PathBuf::from("/tmp/empty-log-root");
    let listing = project_listing(&log_root, vec![]);
    let out = format_projects(&listing);
    let first_line = out.lines().next().expect("header line must exist");
    assert!(
        first_line.contains("(0 projects)"),
        "empty log root must still print `(0 projects)` header; got: {first_line}"
    );
}

#[test]
fn format_projects_row_session_count_singular_vs_plural() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(
        &log_root,
        vec![
            project_row(
                "-tmp-one",
                Some("/tmp/one"),
                CwdSource::FirstRecord,
                1,
                Some(ts(2026, 5, 22, 18, 30)),
            ),
            project_row(
                "-tmp-many",
                Some("/tmp/many"),
                CwdSource::FirstRecord,
                42,
                Some(ts(2026, 5, 22, 18, 30)),
            ),
        ],
    );
    let out = format_projects(&listing);
    assert!(
        out.contains("1 session"),
        "session_count==1 must render as `1 session` (singular); got:\n{out}"
    );
    assert!(
        out.contains("42 sessions"),
        "session_count==42 must render as `42 sessions` (plural); got:\n{out}"
    );
}

#[test]
fn format_projects_latest_timestamp_iso_with_z_suffix() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(
        &log_root,
        vec![project_row(
            "-tmp-dated",
            Some("/tmp/dated"),
            CwdSource::FirstRecord,
            2,
            Some(ts(2026, 5, 22, 18, 30)),
        )],
    );
    let out = format_projects(&listing);
    assert!(
        out.contains("2026-05-22T18:30:00Z"),
        "latest timestamp must be ISO 8601 with `Z` suffix; got:\n{out}"
    );
}

#[test]
fn format_projects_latest_none_renders_word_none() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(
        &log_root,
        vec![project_row(
            "-tmp-empty",
            Some("/tmp/empty"),
            CwdSource::ReconstructedFromEncodedCwd,
            0,
            None,
        )],
    );
    let out = format_projects(&listing);
    assert!(
        out.contains("none"),
        "latest_session_at == None must render as the literal word `none`; got:\n{out}"
    );
}

// =======================================================================
// Step 5: format_sessions_default (git-log-style)
// =======================================================================

#[test]
fn format_sessions_default_header_has_project_encoded_and_total() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        3,
        0,
        vec![fixture_session(
            "s-uuid-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("a title"),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("/tmp/proj"),
        "header must name the project's ground-truth cwd; got:\n{out}"
    );
    assert!(
        out.contains("-tmp-proj"),
        "header must name the encoded_cwd; got:\n{out}"
    );
    assert!(
        out.contains("3 total"),
        "header must report the project's total session count as `N total`; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_shows_showing_marker_only_when_truncated() {
    let tmp = TempDir::new().unwrap();
    // Untruncated: session_count == sessions.len(), limit == 0.
    let untruncated = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            "s-1",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("t"),
        )],
    );
    let out_untrunc =
        format_sessions_default(&untruncated, make_resolver(tmp.path().to_path_buf()));
    assert!(
        !out_untrunc.contains("showing"),
        "header must omit `(showing N)` when no truncation occurred; got:\n{out_untrunc}"
    );

    // Truncated: session_count > sessions.len(), limit set.
    let truncated = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        5,
        1,
        vec![fixture_session(
            "s-1",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("t"),
        )],
    );
    let out_trunc = format_sessions_default(&truncated, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out_trunc.contains("(showing 1)"),
        "header must include `(showing N)` when truncation occurred; got:\n{out_trunc}"
    );
}

#[test]
fn format_sessions_default_block_carries_full_uuid_status_and_date() {
    let tmp = TempDir::new().unwrap();
    let sid = "cbb44fe2-744e-4aee-a42d-fe87703da4b3";
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            sid,
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("gh-cli skill orientation"),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains(&format!("session {sid}")),
        "default block must carry the FULL session UUID on a `session <uuid>` header; got:\n{out}"
    );
    assert!(
        out.contains("Status: success"),
        "default block must carry `Status: <status>` lowercase; got:\n{out}"
    );
    assert!(
        out.contains("Date:") && out.contains("2026-05-22T18:30:00Z"),
        "default block must carry `Date:` with ISO 8601 timestamp; got:\n{out}"
    );
    assert!(
        out.contains("gh-cli skill orientation"),
        "default block must carry the title body; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_subagent_block_only_when_subagents_present() {
    let tmp = TempDir::new().unwrap();
    // No subagents on this session — `Subagents` sub-block must be absent.
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            "s-noagents",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("a"),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        !out.contains("Subagents"),
        "sessions with zero subagents must omit the `Subagents` sub-block; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_subagent_block_uses_box_drawing_chars() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-tree";
    // Pre-stage meta.json sidecars so the continuation lines carry real
    // descriptions rather than the `(no description)` placeholder.
    write_meta_json(tmp.path(), sid, "agent-1", "Explore the codebase");
    write_meta_json(tmp.path(), sid, "agent-2", "Phase 1 implementation plan");
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            sid,
            Path::new("/tmp/log/s-tree.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            Some("two agents"),
            None,
            0,
            false,
            vec![
                subagent(
                    "agent-1",
                    "Explore",
                    "tu-1",
                    "completed",
                    Some(11_600),
                    Some(20_792),
                    Some(4),
                    Some(task_stats(2, 1, 4, 0)),
                ),
                subagent(
                    "agent-2",
                    "Plan",
                    "tu-2",
                    "completed",
                    Some(7_200),
                    Some(14_530),
                    Some(12),
                    Some(task_stats(12, 0, 0, 0)),
                ),
            ],
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("Subagents (2):"),
        "subagent sub-block header must read `Subagents (N):`; got:\n{out}"
    );
    assert!(
        out.contains("├─"),
        "subagent tree must use `├─` for non-terminal rows; got:\n{out}"
    );
    assert!(
        out.contains("└─"),
        "subagent tree must use `└─` for the terminal row; got:\n{out}"
    );
    assert!(
        out.contains("Explore the codebase"),
        "continuation line must render the meta.json description; got:\n{out}"
    );
    assert!(
        out.contains("Phase 1 implementation plan"),
        "continuation line must render the second subagent's description; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_subagent_row_renders_duration_tokens_and_dominant_tool() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-cols";
    write_meta_json(tmp.path(), sid, "agent-1", "desc");
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            sid,
            Path::new("/tmp/log/s-cols.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            Some("t"),
            None,
            0,
            false,
            vec![subagent(
                "agent-1",
                "Explore",
                "tu-1",
                "completed",
                Some(11_600),
                Some(20_792),
                Some(4),
                Some(task_stats(0, 0, 4, 0)), // bash dominates
            )],
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("11.6s"),
        "duration must format as seconds with one decimal (`11600 ms` -> `11.6s`); got:\n{out}"
    );
    assert!(
        out.contains("20792 tok"),
        "tokens must render as `<n> tok`; got:\n{out}"
    );
    assert!(
        out.contains("bash=4"),
        "dominant tool tag must render as `<cat>=<count>`; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_subagent_no_description_when_meta_missing() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-nodesc";
    // Deliberately do NOT call write_meta_json — meta.json is missing.
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            sid,
            Path::new("/tmp/log/s-nodesc.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            Some("t"),
            None,
            0,
            false,
            vec![subagent(
                "agent-x",
                "Explore",
                "tu-x",
                "completed",
                Some(1000),
                Some(100),
                Some(1),
                Some(empty_tool_stats()),
            )],
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("(no description)"),
        "missing meta.json sidecar must render the literal `(no description)` placeholder; got:\n{out}"
    );
}

// ---- Summary inference (formatter-side wrapping) -----------------------

#[test]
fn format_sessions_default_summary_uses_ai_title_verbatim() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            "s-1",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("My session title"),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("My session title"),
        "ai_title must render verbatim; got:\n{out}"
    );
    assert!(
        !out.contains("(untitled)"),
        "(untitled) wrapper must NOT appear when ai_title is set; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_summary_wraps_untitled_excerpt_when_no_title() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            "s-1",
            Path::new("/tmp/log/s-1.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            None,
            Some("let's continue with phase 2"),
            0,
            false,
            Vec::new(),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains(r#"(untitled) "let's continue with phase 2""#),
        "no ai_title must wrap excerpt as `(untitled) \"...\"`; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_summary_could_not_read_session_when_unknown() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            "s-unread",
            Path::new("/tmp/log/s-unread.jsonl"),
            None,
            None,
            SessionStatus::Unknown,
            None,
            None,
            0,
            false,
            Vec::new(),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("(could not read session)"),
        "Unknown status with no ai_title and no excerpt must render `(could not read session)`; got:\n{out}"
    );
}

#[test]
fn format_sessions_default_summary_no_content_when_all_fields_empty() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            "s-blank",
            Path::new("/tmp/log/s-blank.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            None,
            None,
            0,
            false,
            Vec::new(),
        )],
    );
    let out = format_sessions_default(&listing, make_resolver(tmp.path().to_path_buf()));
    assert!(
        out.contains("(no content)"),
        "no ai_title, no excerpt, no read_error must render `(no content)`; got:\n{out}"
    );
}

// =======================================================================
// Step 5: format_sessions_oneline (compact)
// =======================================================================

#[test]
fn format_sessions_oneline_uuid_truncated_to_eight_chars() {
    let tmp = TempDir::new().unwrap();
    let sid = "cbb44fe2-744e-4aee-a42d-fe87703da4b3";
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            sid,
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("gh-cli skill orientation"),
        )],
    );
    let out = format_sessions_oneline(&listing);
    assert!(
        out.contains("cbb44fe2"),
        "oneline row must carry the first 8 chars of the UUID; got:\n{out}"
    );
    // Full UUID must NOT appear in any row body — that's the abbreviation
    // contract from § Subcommand `tracks PROJECT` column rules.
    let body_lines: Vec<&str> = out
        .lines()
        .filter(|l| {
            !l.starts_with("Project")
                && !l.starts_with("Encoded")
                && !l.starts_with("Sessions")
                && !l.is_empty()
        })
        .collect();
    for line in &body_lines {
        assert!(
            !line.contains(sid),
            "oneline row must NOT contain the full UUID (abbreviation is the whole point); got line: {line}"
        );
    }
}

#[test]
fn format_sessions_oneline_subagent_count_tag_when_nonzero() {
    let tmp = TempDir::new().unwrap();
    let sid = "abcd1234-0000-0000-0000-000000000000";
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            sid,
            Path::new("/tmp/log/abcd.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            Some("t"),
            None,
            0,
            false,
            vec![
                subagent(
                    "a1",
                    "Explore",
                    "tu1",
                    "completed",
                    Some(1000),
                    Some(100),
                    Some(1),
                    Some(empty_tool_stats()),
                ),
                subagent(
                    "a2",
                    "Plan",
                    "tu2",
                    "completed",
                    Some(1000),
                    Some(100),
                    Some(1),
                    Some(empty_tool_stats()),
                ),
                subagent(
                    "a3",
                    "general-purpose",
                    "tu3",
                    "completed",
                    Some(1000),
                    Some(100),
                    Some(1),
                    Some(empty_tool_stats()),
                ),
            ],
        )],
    );
    let out = format_sessions_oneline(&listing);
    assert!(
        out.contains("[3 subagents]"),
        "oneline row with N>1 subagents must append `[N subagents]`; got:\n{out}"
    );
}

#[test]
fn format_sessions_oneline_singular_subagent_uses_singular_noun() {
    let tmp = TempDir::new().unwrap();
    let sid = "aaaa1111-0000-0000-0000-000000000000";
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            sid,
            Path::new("/tmp/log/aaaa.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Aborted,
            None,
            Some("a single subagent here"),
            0,
            false,
            vec![subagent(
                "a1", "Explore", "tu1", "aborted", None, None, None, None,
            )],
        )],
    );
    let out = format_sessions_oneline(&listing);
    assert!(
        out.contains("[1 subagent]"),
        "oneline row with exactly 1 subagent must use `[1 subagent]` singular; got:\n{out}"
    );
    assert!(
        !out.contains("[1 subagents]"),
        "must NOT pluralise the count==1 form; got:\n{out}"
    );
}

#[test]
fn format_sessions_oneline_no_tag_when_zero_subagents() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            "ebd6c5f1-0000-0000-0000-000000000000",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("wire ccturn spin subcommand"),
        )],
    );
    let out = format_sessions_oneline(&listing);
    assert!(
        !out.contains("subagent"),
        "zero-subagent row must NOT carry any `[N subagent(s)]` tag; got:\n{out}"
    );
}

#[test]
fn format_sessions_oneline_drops_subagent_tree() {
    let tmp = TempDir::new().unwrap();
    write_meta_json(
        tmp.path(),
        "s-tree",
        "agent-1",
        "this description must not appear",
    );
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            "s-tree",
            Path::new("/tmp/log/s-tree.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            Some("t"),
            None,
            0,
            false,
            vec![subagent(
                "agent-1",
                "Explore",
                "tu1",
                "completed",
                Some(1000),
                Some(100),
                Some(1),
                Some(empty_tool_stats()),
            )],
        )],
    );
    let out = format_sessions_oneline(&listing);
    assert!(
        !out.contains("├─") && !out.contains("└─"),
        "oneline mode must NOT render the box-drawing subagent tree; got:\n{out}"
    );
    assert!(
        !out.contains("this description must not appear"),
        "oneline mode must NOT read meta.json descriptions; got:\n{out}"
    );
}

// =======================================================================
// Step 6: format_projects_json
// =======================================================================

#[test]
fn format_projects_json_emits_snake_case_keys_and_kebab_case_cwd_source() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(
        &log_root,
        vec![project_row(
            "-tmp-a",
            Some("/tmp/a"),
            CwdSource::FirstRecord,
            3,
            Some(ts(2026, 5, 22, 18, 30)),
        )],
    );
    let json: Value = serde_json::from_str(&format_projects_json(&listing))
        .expect("format_projects_json must return a single JSON object");
    assert!(
        json.get("log_root").is_some(),
        "top-level key `log_root` missing"
    );
    assert!(
        json.get("projects").is_some(),
        "top-level key `projects` missing"
    );
    let rows = json["projects"].as_array().expect("projects must be array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    for key in [
        "encoded_cwd",
        "project_cwd",
        "cwd_source",
        "session_count",
        "latest_session_at",
    ] {
        assert!(
            row.get(key).is_some(),
            "row must carry snake_case key `{key}`; got {row}"
        );
    }
    assert_eq!(row["cwd_source"], "first-record");
}

#[test]
fn format_projects_json_emits_empty_array_when_no_projects() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(&log_root, vec![]);
    let json: Value = serde_json::from_str(&format_projects_json(&listing)).expect("must parse");
    let projects = json
        .get("projects")
        .expect("projects key must be present even when empty");
    assert!(
        projects.is_array() && projects.as_array().unwrap().is_empty(),
        "empty projects must serialise as `[]`, never omitted nor null; got {projects}"
    );
}

#[test]
fn format_projects_json_cwd_source_reconstructed_uses_kebab_case() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(
        &log_root,
        vec![project_row(
            "-tmp-r",
            Some("/tmp/r"),
            CwdSource::ReconstructedFromEncodedCwd,
            0,
            None,
        )],
    );
    let json: Value = serde_json::from_str(&format_projects_json(&listing)).unwrap();
    assert_eq!(
        json["projects"][0]["cwd_source"],
        "reconstructed-from-encoded-cwd"
    );
    assert!(json["projects"][0]["latest_session_at"].is_null());
}

#[test]
fn format_projects_json_is_single_line_non_pretty() {
    let log_root = PathBuf::from("/tmp/root");
    let listing = project_listing(
        &log_root,
        vec![project_row(
            "-tmp-a",
            Some("/tmp/a"),
            CwdSource::FirstRecord,
            1,
            Some(ts(2026, 5, 22, 18, 30)),
        )],
    );
    let s = format_projects_json(&listing);
    assert!(
        !s.contains('\n'),
        "JSON output must be non-pretty (no newlines); got:\n{s}"
    );
}

// =======================================================================
// Step 6: format_sessions_json
// =======================================================================

#[test]
fn format_sessions_json_emits_top_level_fields_and_status_snake_case() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        2,
        0,
        vec![
            session_row(
                "s-1",
                Path::new("/tmp/log/s-1.jsonl"),
                Some(ts_string(2026, 5, 22, 18, 30)),
                Some(ts_string(2026, 5, 22, 18, 30)),
                SessionStatus::Success,
                Some("title"),
                None,
                0,
                false,
                Vec::new(),
            ),
            session_row(
                "s-2",
                Path::new("/tmp/log/s-2.jsonl"),
                None,
                None,
                SessionStatus::Unknown,
                None,
                None,
                0,
                false,
                Vec::new(),
            ),
        ],
    );
    let json: Value = serde_json::from_str(&format_sessions_json(&listing))
        .expect("must parse as single JSON object");
    for key in [
        "log_root",
        "encoded_cwd",
        "project_cwd",
        "cwd_source",
        "session_count",
        "limit",
        "sessions",
    ] {
        assert!(
            json.get(key).is_some(),
            "top-level key `{key}` missing from {json}"
        );
    }
    let rows = json["sessions"].as_array().expect("sessions must be array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["status"], "success");
    assert_eq!(rows[1]["status"], "unknown");
}

#[test]
fn format_sessions_json_session_row_has_all_required_fields() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            "s-1",
            Path::new("/tmp/log/s-1.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Error,
            Some("title"),
            Some("excerpt"),
            2,
            true,
            Vec::new(),
        )],
    );
    let json: Value = serde_json::from_str(&format_sessions_json(&listing)).unwrap();
    let row = &json["sessions"][0];
    for key in [
        "session_id",
        "log_path",
        "started_at",
        "ended_at",
        "status",
        "ai_title",
        "first_user_message_excerpt",
        "open_tool_uses",
        "last_tool_result_is_error",
        "subagents",
    ] {
        assert!(
            row.get(key).is_some(),
            "session row must carry snake_case key `{key}`; got {row}"
        );
    }
    assert_eq!(row["status"], "error");
    assert!(row["last_tool_result_is_error"].as_bool().unwrap());
}

#[test]
fn format_sessions_json_empty_sessions_and_subagents_arrays_are_emitted() {
    let tmp = TempDir::new().unwrap();
    // Empty sessions case.
    let listing_no_sessions = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        0,
        0,
        Vec::new(),
    );
    let json_a: Value = serde_json::from_str(&format_sessions_json(&listing_no_sessions)).unwrap();
    let sessions = json_a
        .get("sessions")
        .expect("`sessions` key must be present even when empty");
    assert!(
        sessions.is_array() && sessions.as_array().unwrap().is_empty(),
        "empty sessions must serialise as `[]`, never omitted nor null; got {sessions}"
    );

    // Empty subagents per session.
    let listing_with_sessions = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            "s-1",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("t"),
        )],
    );
    let json_b: Value =
        serde_json::from_str(&format_sessions_json(&listing_with_sessions)).unwrap();
    let subs = json_b["sessions"][0]
        .get("subagents")
        .expect("session row must carry `subagents` key even when empty");
    assert!(
        subs.is_array() && subs.as_array().unwrap().is_empty(),
        "empty subagents must serialise as `[]`, never omitted nor null; got {subs}"
    );
}

#[test]
fn format_sessions_json_subagents_carry_payload_fields_and_tool_stats() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![session_row(
            "s-sub",
            Path::new("/tmp/log/s-sub.jsonl"),
            Some(ts_string(2026, 5, 22, 18, 30)),
            Some(ts_string(2026, 5, 22, 18, 30)),
            SessionStatus::Success,
            Some("t"),
            None,
            0,
            false,
            vec![subagent(
                "agent-x",
                "Explore",
                "tu-x",
                "completed",
                Some(5000),
                Some(12_000),
                Some(3),
                Some(task_stats(2, 1, 0, 0)),
            )],
        )],
    );
    let json: Value = serde_json::from_str(&format_sessions_json(&listing)).unwrap();
    let sub = &json["sessions"][0]["subagents"][0];
    for key in [
        "agent_id",
        "agent_type",
        "tool_use_id",
        "status",
        "total_duration_ms",
        "total_tokens",
        "total_tool_use_count",
        "tool_stats",
    ] {
        assert!(
            sub.get(key).is_some(),
            "subagent must carry snake_case key `{key}`; got {sub}"
        );
    }
    let stats = &sub["tool_stats"];
    for key in [
        "read_count",
        "search_count",
        "bash_count",
        "edit_file_count",
        "lines_added",
        "lines_removed",
        "other_tool_count",
    ] {
        assert!(
            stats.get(key).is_some(),
            "tool_stats must carry snake_case key `{key}`; got {stats}"
        );
    }
    assert_eq!(stats["read_count"], 2);
    assert_eq!(stats["bash_count"], 0);
}

#[test]
fn format_sessions_json_limit_zero_means_no_limit() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        3,
        0,
        vec![
            fixture_session(
                "s-1",
                ts_string(2026, 5, 22, 18, 30),
                SessionStatus::Success,
                Some("a"),
            ),
            fixture_session(
                "s-2",
                ts_string(2026, 5, 22, 17, 30),
                SessionStatus::Success,
                Some("b"),
            ),
            fixture_session(
                "s-3",
                ts_string(2026, 5, 22, 16, 30),
                SessionStatus::Success,
                Some("c"),
            ),
        ],
    );
    let json: Value = serde_json::from_str(&format_sessions_json(&listing)).unwrap();
    assert_eq!(
        json["limit"], 0,
        "limit==0 in the struct must round-trip as JSON `0` (no-limit sentinel)"
    );
    assert_eq!(json["session_count"], 3);
    assert_eq!(json["sessions"].as_array().unwrap().len(), 3);
}

#[test]
fn format_sessions_json_is_single_line_non_pretty() {
    let tmp = TempDir::new().unwrap();
    let listing = session_listing(
        tmp.path(),
        "-tmp-proj",
        Some("/tmp/proj"),
        CwdSource::FirstRecord,
        1,
        0,
        vec![fixture_session(
            "s-1",
            ts_string(2026, 5, 22, 18, 30),
            SessionStatus::Success,
            Some("t"),
        )],
    );
    let s = format_sessions_json(&listing);
    assert!(
        !s.contains('\n'),
        "JSON output must be non-pretty (no newlines); got:\n{s}"
    );
}
