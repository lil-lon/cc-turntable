// Phase 2 Step 3 + 4 — Session enumeration module and per-session metadata
// extractor.
//
// Step 3: `pub fn list_sessions(log_root, encoded_cwd, limit) ->
// anyhow::Result<SessionListing>` in `src/list/sessions.rs`.
// Step 4: `pub(crate) fn extract_session_metadata(path) -> SessionMetadata`
// in `src/list/metadata.rs`, called from `list_sessions`.
//
// These two modules are tightly coupled, so this file exercises both via
// the public `list_sessions` entry point. The extractor is reached
// indirectly through the SessionRow values it builds.
//
// Per the design doc:
//   * Status ladder (first-match wins): read_error -> unknown,
//     open_tool_uses > 0 -> aborted, last_tool_result_is_error -> error,
//     otherwise success.
//   * Summary inference is stored as raw `ai_title` /
//     `first_user_message_excerpt` fields; the wrapping into
//     "(untitled) ..." / "(could not read session)" / "(no content)"
//     lives in the formatter (Step 5) and is NOT tested here.
//   * Excerpt filter skips records with `isMeta == true`,
//     `sourceToolUseID` set, or content starting with `<command-name>`.
//   * Excerpt truncates to 80 chars with an `…` ellipsis on overflow.
//   * Subagents: each Task tool_use yields a SubagentSummary; a dangling
//     Task tool_use emits `status: "aborted"` with all numeric fields
//     None.

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

use ccturn::list::sessions::{SessionListing, SessionRow, SessionStatus, list_sessions};

// ---- Fixture helpers ---------------------------------------------------

// Build `<log_root>/<encoded_cwd>/`, write `<session_id>.jsonl` with body.
fn write_session(log_root: &Path, encoded_cwd: &str, session_id: &str, body: &str) -> PathBuf {
    let project_dir = log_root.join(encoded_cwd);
    fs::create_dir_all(&project_dir).expect("create project dir");
    let jsonl = project_dir.join(format!("{session_id}.jsonl"));
    fs::write(&jsonl, body).expect("write session jsonl");
    jsonl
}

// Build a synthetic mtime so the limit-after-sort test does not have to
// wait on real wall-clock differences when sorting falls back to mtime
// (which it does not for started_at, but the helper is handy if needed).
const MTIME_EPOCH: u64 = 1_700_000_000;

fn fixed_mtime(offset_secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(MTIME_EPOCH + offset_secs)
}

fn set_mtime(path: &Path, mtime: SystemTime) {
    let file = File::options()
        .write(true)
        .open(path)
        .expect("open file to set mtime");
    let times = FileTimes::new().set_modified(mtime);
    file.set_times(times).expect("set mtime");
}

// ---- JSONL record helpers (one record per call, terminated with `\n`) --

fn user_text_record(session_id: &str, ts: &str, uuid: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","message":{{"role":"user","content":{json_text}}}}}
"#,
        json_text = serde_json::to_string(text).expect("text must be JSON-encodable"),
    )
}

fn user_text_record_meta(session_id: &str, ts: &str, uuid: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","isMeta":true,"message":{{"role":"user","content":{json_text}}}}}
"#,
        json_text = serde_json::to_string(text).expect("text must be JSON-encodable"),
    )
}

fn user_text_record_skill_body(
    session_id: &str,
    ts: &str,
    uuid: &str,
    source_tool_use_id: &str,
    text: &str,
) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","sourceToolUseID":"{source_tool_use_id}","message":{{"role":"user","content":{json_text}}}}}
"#,
        json_text = serde_json::to_string(text).expect("text must be JSON-encodable"),
    )
}

fn assistant_tool_use_record(
    session_id: &str,
    ts: &str,
    uuid: &str,
    tool_use_id: &str,
    name: &str,
    input_json: &str,
) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{tool_use_id}","name":"{name}","input":{input_json}}}]}}}}
"#,
    )
}

// User-side tool_result. `is_error` controls the boolean flag.
fn user_tool_result_record(
    session_id: &str,
    ts: &str,
    uuid: &str,
    tool_use_id: &str,
    is_error: bool,
) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{tool_use_id}","content":"result","is_error":{is_error}}}]}}}}
"#,
    )
}

// User-side tool_result that also carries the top-level `toolUseResult`
// payload (matched-Task subagent shape).
fn user_task_result_record(
    session_id: &str,
    ts: &str,
    uuid: &str,
    tool_use_id: &str,
    agent_id: &str,
    agent_type: &str,
) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{tool_use_id}","content":"explored","is_error":false}}]}},"toolUseResult":{{"status":"completed","agentId":"{agent_id}","agentType":"{agent_type}","content":[{{"type":"text","text":"explored"}}],"totalDurationMs":5000,"totalTokens":12000,"totalToolUseCount":3,"toolStats":{{"readCount":2,"searchCount":1,"bashCount":0,"editFileCount":0,"linesAdded":0,"linesRemoved":0,"otherToolCount":0}}}}}}
"#,
    )
}

fn ai_title_record(session_id: &str, ts: &str, uuid: &str, title: &str) -> String {
    format!(
        r#"{{"type":"ai-title","uuid":"{uuid}","sessionId":"{session_id}","cwd":"/tmp/test","timestamp":"{ts}","title":{json_title}}}
"#,
        json_title = serde_json::to_string(title).expect("title must be JSON-encodable"),
    )
}

fn file_history_snapshot_record(session_id: &str, ts: &str, uuid: &str) -> String {
    format!(
        r#"{{"type":"file-history-snapshot","uuid":"{uuid}","sessionId":"{session_id}","snapshot":{{"messageId":"{uuid}","trackedFileBackups":{{}},"timestamp":"{ts}"}},"isSnapshotUpdate":false}}
"#,
    )
}

// Locate a row by session_id. Panics on miss for clear failure messages.
fn row_by_session_id<'a>(listing: &'a SessionListing, session_id: &str) -> &'a SessionRow {
    listing
        .sessions
        .iter()
        .find(|r| r.session_id == session_id)
        .unwrap_or_else(|| {
            panic!(
                "no session row with id `{session_id}`; got {:?}",
                listing
                    .sessions
                    .iter()
                    .map(|r| &r.session_id)
                    .collect::<Vec<_>>()
            )
        })
}

// ---- Step 3: list-level concerns ---------------------------------------

#[test]
fn list_sessions_missing_project_returns_err() {
    let tmp = TempDir::new().unwrap();
    let result = list_sessions(tmp.path(), "-tmp-does-not-exist", None);
    assert!(
        result.is_err(),
        "missing project must return Err; got Ok({:?})",
        result.ok().map(|l| l.sessions.len())
    );
}

#[test]
fn list_sessions_empty_project_yields_empty_sessions() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("-tmp-empty")).unwrap();
    let listing =
        list_sessions(tmp.path(), "-tmp-empty", None).expect("empty project must succeed");
    assert!(
        listing.sessions.is_empty(),
        "empty project must yield zero session rows; got {} rows",
        listing.sessions.len()
    );
    assert_eq!(listing.session_count, 0);
    assert_eq!(listing.encoded_cwd, "-tmp-empty");
}

#[test]
fn list_sessions_session_id_is_jsonl_stem() {
    let tmp = TempDir::new().unwrap();
    let sid = "abc-123-uuid";
    write_session(
        tmp.path(),
        "-tmp-stem",
        sid,
        &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hello"),
    );
    let listing = list_sessions(tmp.path(), "-tmp-stem", None).expect("listing must succeed");
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(
        listing.sessions[0].session_id, sid,
        "session_id must be the .jsonl filename without the extension"
    );
}

#[test]
fn list_sessions_sorts_by_started_at_descending() {
    let tmp = TempDir::new().unwrap();
    // Three sessions with distinct started_at; expected order is newest first.
    write_session(
        tmp.path(),
        "-tmp-sort",
        "s-old",
        &user_text_record("s-old", "2026-05-18T09:00:00.000Z", "u1", "old"),
    );
    write_session(
        tmp.path(),
        "-tmp-sort",
        "s-mid",
        &user_text_record("s-mid", "2026-05-19T09:00:00.000Z", "u1", "mid"),
    );
    write_session(
        tmp.path(),
        "-tmp-sort",
        "s-new",
        &user_text_record("s-new", "2026-05-20T09:00:00.000Z", "u1", "new"),
    );
    let listing = list_sessions(tmp.path(), "-tmp-sort", None).expect("listing must succeed");
    let order: Vec<&str> = listing
        .sessions
        .iter()
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(
        order,
        vec!["s-new", "s-mid", "s-old"],
        "sessions must sort by started_at desc (most recent first)"
    );
}

#[test]
fn list_sessions_sort_puts_none_started_at_last_with_session_id_tiebreak() {
    let tmp = TempDir::new().unwrap();
    // One session with a known started_at.
    write_session(
        tmp.path(),
        "-tmp-nulls",
        "s-dated",
        &user_text_record("s-dated", "2026-05-20T09:00:00.000Z", "u1", "x"),
    );
    // Two sessions with NO records at all → started_at == None. They must
    // sort last and amongst themselves by session_id ascending.
    write_session(tmp.path(), "-tmp-nulls", "s-zzz", "");
    write_session(tmp.path(), "-tmp-nulls", "s-aaa", "");
    let listing = list_sessions(tmp.path(), "-tmp-nulls", None).expect("listing must succeed");
    let order: Vec<&str> = listing
        .sessions
        .iter()
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(
        order,
        vec!["s-dated", "s-aaa", "s-zzz"],
        "started_at=None rows must sort last with session_id asc tiebreak"
    );
}

#[test]
fn list_sessions_limit_truncates_after_sort_and_session_count_is_pretruncation() {
    let tmp = TempDir::new().unwrap();
    for (sid, ts) in [
        ("s-1", "2026-05-18T09:00:00.000Z"),
        ("s-2", "2026-05-19T09:00:00.000Z"),
        ("s-3", "2026-05-20T09:00:00.000Z"),
        ("s-4", "2026-05-21T09:00:00.000Z"),
    ] {
        write_session(
            tmp.path(),
            "-tmp-limit",
            sid,
            &user_text_record(sid, ts, "u1", "x"),
        );
    }
    let listing = list_sessions(tmp.path(), "-tmp-limit", Some(2)).expect("listing must succeed");
    assert_eq!(
        listing.session_count, 4,
        "session_count must report the pre-truncation total"
    );
    assert_eq!(
        listing.sessions.len(),
        2,
        "limit=2 must truncate the sessions array to 2 rows"
    );
    let order: Vec<&str> = listing
        .sessions
        .iter()
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(
        order,
        vec!["s-4", "s-3"],
        "truncation must keep the two MOST RECENT sessions"
    );
    assert_eq!(listing.limit, 2, "listing.limit must echo the applied cap");
}

#[test]
fn list_sessions_no_limit_keeps_all_and_listing_limit_is_zero() {
    let tmp = TempDir::new().unwrap();
    for sid in ["s-a", "s-b", "s-c"] {
        write_session(
            tmp.path(),
            "-tmp-nolimit",
            sid,
            &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "x"),
        );
    }
    let listing = list_sessions(tmp.path(), "-tmp-nolimit", None).expect("listing must succeed");
    assert_eq!(
        listing.sessions.len(),
        3,
        "limit=None must keep every session in the project"
    );
    assert_eq!(
        listing.limit, 0,
        "listing.limit==0 means `no limit applied` per JSON-schema convention"
    );
    assert_eq!(listing.session_count, 3);
}

#[test]
fn list_sessions_log_root_and_encoded_cwd_echo_inputs() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("-tmp-echo")).unwrap();
    let listing = list_sessions(tmp.path(), "-tmp-echo", None).expect("listing must succeed");
    assert_eq!(listing.log_root, tmp.path());
    assert_eq!(listing.encoded_cwd, "-tmp-echo");
}

#[test]
fn list_sessions_unreadable_jsonl_surfaces_status_unknown_not_propagated() {
    // Mac/Linux only: we strip read permissions on the JSONL so the open
    // path fails. The whole listing must still succeed (Err is NEVER raised
    // for a per-session failure); the affected row carries status=unknown.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let jsonl = write_session(
            tmp.path(),
            "-tmp-unreadable",
            "s-broken",
            &user_text_record("s-broken", "2026-05-19T09:00:00.000Z", "u1", "hello"),
        );
        // Also write a readable sibling so we can confirm the listing
        // does not abort wholesale on the broken file.
        write_session(
            tmp.path(),
            "-tmp-unreadable",
            "s-ok",
            &user_text_record("s-ok", "2026-05-19T09:00:01.000Z", "u1", "ok"),
        );
        fs::set_permissions(&jsonl, fs::Permissions::from_mode(0o000))
            .expect("chmod 000 to simulate unreadable jsonl");

        let listing_result = list_sessions(tmp.path(), "-tmp-unreadable", None);

        // Restore permissions BEFORE asserting so TempDir cleanup works
        // even if the assertions panic.
        let _ = fs::set_permissions(&jsonl, fs::Permissions::from_mode(0o644));

        let listing = listing_result.expect("per-session failures must NOT propagate as Err");
        let broken = row_by_session_id(&listing, "s-broken");
        assert_eq!(
            broken.status,
            SessionStatus::Unknown,
            "unreadable JSONL must surface as status=unknown"
        );
        let ok = row_by_session_id(&listing, "s-ok");
        assert_eq!(
            ok.status,
            SessionStatus::Success,
            "the readable sibling must still produce a real row"
        );
    }
}

// ---- Step 4: status ladder ---------------------------------------------

#[test]
fn status_success_on_clean_session() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-clean";
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hello")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-1",
            "Read",
            r#"{"path":"/tmp/x"}"#,
        )
        + &user_tool_result_record(sid, "2026-05-19T09:00:02.000Z", "u3", "tu-1", false);
    write_session(tmp.path(), "-tmp-status", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-status", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(row.status, SessionStatus::Success);
    assert!(
        !row.last_tool_result_is_error,
        "last_tool_result_is_error must be false on a clean ending"
    );
    assert_eq!(row.open_tool_uses, 0);
}

#[test]
fn status_error_when_last_tool_result_is_error() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-err";
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hello")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-err",
            "Bash",
            r#"{"command":"x"}"#,
        )
        + &user_tool_result_record(sid, "2026-05-19T09:00:02.000Z", "u3", "tu-err", true);
    write_session(tmp.path(), "-tmp-status", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-status", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(row.status, SessionStatus::Error);
    assert!(row.last_tool_result_is_error);
    assert_eq!(row.open_tool_uses, 0);
}

#[test]
fn status_aborted_when_tool_use_dangles() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-abort";
    // tool_use without matching tool_result → open_tool_uses > 0 at EOF.
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hello")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-dangle",
            "Bash",
            r#"{"command":"sleep 9999"}"#,
        );
    write_session(tmp.path(), "-tmp-status", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-status", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(row.status, SessionStatus::Aborted);
    assert!(
        row.open_tool_uses > 0,
        "dangling tool_use must surface in open_tool_uses; got {}",
        row.open_tool_uses
    );
}

#[test]
fn status_aborted_takes_precedence_over_error() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-precedence";
    // Earlier resolved tool_use with is_error: true, plus a later
    // unresolved tool_use. Per the spec's heuristic boundaries note:
    // "aborted takes precedence over error".
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hi")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-err",
            "Bash",
            r#"{"command":"x"}"#,
        )
        + &user_tool_result_record(sid, "2026-05-19T09:00:02.000Z", "u3", "tu-err", true)
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:03.000Z",
            "u4",
            "tu-dangle",
            "Bash",
            r#"{"command":"sleep 9999"}"#,
        );
    write_session(tmp.path(), "-tmp-status", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-status", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.status,
        SessionStatus::Aborted,
        "a dangling tool_use must demote to aborted even when an earlier tool_result errored"
    );
}

#[test]
fn status_success_when_intermediate_error_followed_by_success() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-recovery";
    // Earlier tool_result is_error=true, but a LATER resolved tool_result
    // is is_error=false. Per the spec: only the LAST tool_result matters,
    // so the row is Success.
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hi")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-bad",
            "Bash",
            r#"{"command":"x"}"#,
        )
        + &user_tool_result_record(sid, "2026-05-19T09:00:02.000Z", "u3", "tu-bad", true)
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:03.000Z",
            "u4",
            "tu-good",
            "Bash",
            r#"{"command":"echo hi"}"#,
        )
        + &user_tool_result_record(sid, "2026-05-19T09:00:04.000Z", "u5", "tu-good", false);
    write_session(tmp.path(), "-tmp-status", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-status", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.status,
        SessionStatus::Success,
        "an intermediate error must NOT demote when the LAST tool_result succeeds"
    );
    assert!(!row.last_tool_result_is_error);
}

// ---- Step 4: summary / excerpt fields ----------------------------------

#[test]
fn ai_title_recorded_verbatim_when_present() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-title";
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hi")
        + &ai_title_record(sid, "2026-05-19T09:00:01.000Z", "u2", "My AI title");
    write_session(tmp.path(), "-tmp-title", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-title", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.ai_title.as_deref(),
        Some("My AI title"),
        "ai_title must be recorded verbatim from the ai-title record's title"
    );
}

#[test]
fn ai_title_last_seen_wins_when_multiple_records() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-titles";
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "hi")
        + &ai_title_record(sid, "2026-05-19T09:00:01.000Z", "u2", "first title")
        + &ai_title_record(sid, "2026-05-19T09:00:02.000Z", "u3", "final title");
    write_session(tmp.path(), "-tmp-titles", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-titles", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.ai_title.as_deref(),
        Some("final title"),
        "when multiple ai-title records exist the LAST one wins"
    );
}

#[test]
fn first_user_message_recorded_when_no_ai_title() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-fum";
    let body = user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "start working");
    write_session(tmp.path(), "-tmp-fum", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-fum", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert!(
        row.ai_title.is_none(),
        "ai_title must be None when no ai-title record is present"
    );
    assert_eq!(
        row.first_user_message_excerpt.as_deref(),
        Some("start working"),
        "first_user_message_excerpt must carry the first user message body"
    );
}

#[test]
fn excerpt_filter_skips_meta_records() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-meta";
    // Meta record FIRST — filter must skip it and pick the second
    // (non-meta) user record's content.
    let body = String::new()
        + &user_text_record_meta(sid, "2026-05-19T09:00:00.000Z", "u1", "<meta-system>")
        + &user_text_record(sid, "2026-05-19T09:00:01.000Z", "u2", "real first message");
    write_session(tmp.path(), "-tmp-meta", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-meta", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.first_user_message_excerpt.as_deref(),
        Some("real first message"),
        "first_user_message excerpt must skip user records with isMeta==true"
    );
}

#[test]
fn excerpt_filter_skips_skill_body_injection() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-skill";
    let body = String::new()
        + &user_text_record_skill_body(
            sid,
            "2026-05-19T09:00:00.000Z",
            "u1",
            "toolu_SKILL_BODY",
            "Launching skill: gh-cli",
        )
        + &user_text_record(sid, "2026-05-19T09:00:01.000Z", "u2", "actual user words");
    write_session(tmp.path(), "-tmp-skill", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-skill", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.first_user_message_excerpt.as_deref(),
        Some("actual user words"),
        "first_user_message excerpt must skip user records with sourceToolUseID set"
    );
}

#[test]
fn excerpt_filter_skips_slash_command_wrapper() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-slash";
    let body = String::new()
        + &user_text_record(
            sid,
            "2026-05-19T09:00:00.000Z",
            "u1",
            "<command-name>/loop</command-name><command-args>5m</command-args>",
        )
        + &user_text_record(sid, "2026-05-19T09:00:01.000Z", "u2", "now the real prompt");
    write_session(tmp.path(), "-tmp-slash", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-slash", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.first_user_message_excerpt.as_deref(),
        Some("now the real prompt"),
        "first_user_message excerpt must skip user records whose content begins with <command-name>"
    );
}

#[test]
fn excerpt_truncated_to_80_chars_with_ellipsis() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-long";
    // 100 chars of `a` — must be cut to 80 chars (including or excluding
    // the ellipsis is implementation-defined; the assertion only pins
    // total char length <= 80 and the trailing ellipsis on overflow).
    let long_message = "a".repeat(100);
    let body = user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", &long_message);
    write_session(tmp.path(), "-tmp-long", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-long", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    let excerpt = row
        .first_user_message_excerpt
        .as_deref()
        .expect("excerpt must be set");
    let codepoints = excerpt.chars().count();
    assert!(
        codepoints <= 80,
        "first_user_message_excerpt must be truncated to <=80 codepoints; got {codepoints}"
    );
    assert!(
        excerpt.ends_with('…'),
        "truncated excerpt must end with the `…` ellipsis; got `{excerpt}`"
    );
}

#[test]
fn no_content_fields_when_jsonl_has_only_snapshot() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-nada";
    // File-history-snapshot only — no user records, no ai-title. The
    // extractor must leave ai_title and first_user_message_excerpt as
    // None. The formatter renders "(no content)" from this state in
    // Step 5; the raw fields are tested here.
    let body = file_history_snapshot_record(sid, "2026-05-19T09:00:00.000Z", "snap-1");
    write_session(tmp.path(), "-tmp-nada", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-nada", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert!(
        row.ai_title.is_none(),
        "ai_title must be None when no ai-title record exists; got {:?}",
        row.ai_title
    );
    assert!(
        row.first_user_message_excerpt.is_none(),
        "first_user_message_excerpt must be None when no qualifying user record exists; got {:?}",
        row.first_user_message_excerpt
    );
}

// ---- Step 4: subagent collection ---------------------------------------

#[test]
fn matched_task_emits_subagent_summary_with_payload_fields() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-task";
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "go")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-task",
            "Task",
            r#"{"subagent_type":"Explore","description":"Explore the codebase"}"#,
        )
        + &user_task_result_record(
            sid,
            "2026-05-19T09:00:02.000Z",
            "u3",
            "tu-task",
            "agent-abc-123",
            "Explore",
        );
    write_session(tmp.path(), "-tmp-task", sid, &body);
    let listing = list_sessions(tmp.path(), "-tmp-task", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.subagents.len(),
        1,
        "session with one Task tool_use must produce exactly one SubagentSummary; got {}",
        row.subagents.len()
    );
    let sub = &row.subagents[0];
    assert_eq!(sub.agent_id, "agent-abc-123");
    assert_eq!(sub.agent_type, "Explore");
    assert_eq!(sub.tool_use_id, "tu-task");
    assert_eq!(sub.status, "completed");
    assert_eq!(sub.total_duration_ms, Some(5000));
    assert_eq!(sub.total_tokens, Some(12000));
    assert_eq!(sub.total_tool_use_count, Some(3));
}

#[test]
fn dangling_task_emits_aborted_subagent_with_null_numerics() {
    let tmp = TempDir::new().unwrap();
    let sid = "s-task-dangle";
    // Task tool_use with no matching tool_result → row emitted with
    // status="aborted" and all numeric fields None per the design doc.
    let body = String::new()
        + &user_text_record(sid, "2026-05-19T09:00:00.000Z", "u1", "go")
        + &assistant_tool_use_record(
            sid,
            "2026-05-19T09:00:01.000Z",
            "u2",
            "tu-dangle",
            "Task",
            r#"{"subagent_type":"Plan","description":"Plan something"}"#,
        );
    write_session(tmp.path(), "-tmp-task-dangle", sid, &body);
    let listing =
        list_sessions(tmp.path(), "-tmp-task-dangle", None).expect("listing must succeed");
    let row = row_by_session_id(&listing, sid);
    assert_eq!(
        row.subagents.len(),
        1,
        "a dangling Task tool_use must still emit a SubagentSummary row"
    );
    let sub = &row.subagents[0];
    assert_eq!(sub.tool_use_id, "tu-dangle");
    assert_eq!(
        sub.status, "aborted",
        "dangling Task subagent must carry status=aborted"
    );
    assert!(
        sub.total_duration_ms.is_none(),
        "dangling Task subagent's total_duration_ms must be None; got {:?}",
        sub.total_duration_ms
    );
    assert!(
        sub.total_tokens.is_none(),
        "dangling Task subagent's total_tokens must be None; got {:?}",
        sub.total_tokens
    );
    assert!(
        sub.total_tool_use_count.is_none(),
        "dangling Task subagent's total_tool_use_count must be None; got {:?}",
        sub.total_tool_use_count
    );
    // Whole-session status is also Aborted because the dangling Task is a
    // dangling tool_use — the same signal that drives row 2 of the ladder.
    assert_eq!(row.status, SessionStatus::Aborted);
}

// ---- Avoid dead-code warnings for helpers not used by every test -------

// `fixed_mtime` / `set_mtime` are kept for future tests that need stamped
// mtimes; suppress the warning while they remain unused at runtime.
#[allow(dead_code)]
fn _keep_mtime_helpers_alive() {
    let _ = (
        fixed_mtime as fn(u64) -> SystemTime,
        set_mtime as fn(&Path, SystemTime),
    );
}
