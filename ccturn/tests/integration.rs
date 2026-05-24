// Phase 1 + Phase 2 integration tests against the `ccturn` binary.
//
// Phase 1: `ccturn spin <session-id>` against the
// `tests/fixtures/projects/-tmp-test-project/` fixture tree.
//
// Phase 2 (Step 7): `ccturn crates` and `ccturn tracks <project>` against the
// `tests/fixtures/projects/-tmp-multi-session/` and `-tmp-empty-project/`
// fixture trees. The Step 7 case list comes from § Implementation > Step 7's
// task list and the eight success criteria under § Success Criteria.
//
// NOTE for the Programmer: main.rs must write the human report to stdout with
// `print!` (NOT `println!`). The formatted report already ends with a newline;
// `println!` would append a spurious trailing blank line and break the
// byte-for-byte snapshot assertion.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

// =========================================================================
// Phase 1 — `ccturn spin` integration (unchanged from pre-Phase-2).
// =========================================================================

fn run_spin(extra_args: &[&str]) -> Output {
    let mut args = vec![
        "spin",
        "integration-session",
        "--log-root",
        "tests/fixtures/projects",
    ];
    args.extend_from_slice(extra_args);
    Command::new(env!("CARGO_BIN_EXE_ccturn"))
        .args(&args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to spawn the ccturn binary")
}

#[test]
fn spin_human_output_matches_snapshot_and_exits_zero() {
    let output = run_spin(&[]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "ccturn spin must exit 0 on a successful report; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be valid UTF-8");
    let expected = include_str!("fixtures/integration-expected-human.txt");
    assert_eq!(
        stdout, expected,
        "human output must match the committed snapshot byte-for-byte"
    );
}

#[test]
fn spin_json_output_is_structurally_correct_and_exits_zero() {
    let output = run_spin(&["--json"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "ccturn spin --json must exit 0 on a successful report; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be valid UTF-8");
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be a single valid JSON object");

    assert_eq!(
        json["project_cwd"], "/tmp/test-project",
        "project_cwd must be the decoded ground-truth path from the first record"
    );

    assert!(
        !json["skills"]
            .as_array()
            .expect("skills must be a JSON array")
            .is_empty(),
        "the fixture exercises one Skill invocation"
    );
    assert!(
        !json["subagents"]
            .as_array()
            .expect("subagents must be a JSON array")
            .is_empty(),
        "the fixture exercises one Task subagent"
    );
    assert!(
        !json["interventions"]
            .as_array()
            .expect("interventions must be a JSON array")
            .is_empty(),
        "the fixture exercises error and mid-stream interventions"
    );

    let categories: Vec<&str> = json["errors"]
        .as_array()
        .expect("errors must be a JSON array")
        .iter()
        .map(|e| {
            e["category"]
                .as_str()
                .expect("each error category is a string")
        })
        .collect();
    for expected in [
        "UserRejection",
        "PermissionDenied",
        "HookBlock",
        "Technical",
    ] {
        assert!(
            categories.contains(&expected),
            "all four error categories must be present; missing `{expected}`, got {categories:?}"
        );
    }
}

// =========================================================================
// Phase 2 — common helpers.
// =========================================================================

const MULTI: &str = "-tmp-multi-session";
const EMPTY: &str = "-tmp-empty-project";
const SUCCESS_SID: &str = "aaaaaaaa-1111-1111-1111-111111111111";
const NODESC_SID: &str = "bbbbbbbb-2222-2222-2222-222222222222";
const ERROR_SID: &str = "cccccccc-3333-3333-3333-333333333333";
const ABORTED_SID: &str = "dddddddd-4444-4444-4444-444444444444";
const UNKNOWN_SID: &str = "eeeeeeee-5555-5555-5555-555555555555";

fn run_ccturn(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ccturn"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to spawn the ccturn binary")
}

fn stdout_str(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout must be valid UTF-8")
}

fn parse_json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout_str(output))
        .expect("--json output must be a single valid JSON object")
}

// =========================================================================
// Phase 2 — `crates` against the fixture log root.
// =========================================================================

#[test]
fn crates_human_lists_phase_2_fixture_projects() {
    let output = run_ccturn(&["crates", "--log-root", "tests/fixtures/projects"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "crates must exit 0 on a readable log root; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_str(&output);
    for encoded in ["-tmp-test-project", MULTI, EMPTY] {
        assert!(
            stdout.contains(encoded),
            "crates human output must list `{encoded}`; got:\n{stdout}"
        );
    }
}

#[test]
fn crates_json_lists_phase_2_fixture_projects() {
    let output = run_ccturn(&["crates", "--json", "--log-root", "tests/fixtures/projects"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "crates --json must exit 0 on a readable log root; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    let projects = json["projects"]
        .as_array()
        .expect("projects must be a JSON array");
    let encoded: Vec<&str> = projects
        .iter()
        .map(|p| {
            p["encoded_cwd"]
                .as_str()
                .expect("each project must carry encoded_cwd")
        })
        .collect();
    for expected in ["-tmp-test-project", MULTI, EMPTY] {
        assert!(
            encoded.contains(&expected),
            "crates --json must list `{expected}` in projects[]; got {encoded:?}"
        );
    }
    // The empty project carries session_count 0 and latest_session_at null.
    let empty_row = projects
        .iter()
        .find(|p| p["encoded_cwd"] == EMPTY)
        .expect("empty project row must be present");
    assert_eq!(empty_row["session_count"], 0);
    assert!(
        empty_row["latest_session_at"].is_null(),
        "empty project must have null latest_session_at; got {empty_row}"
    );
}

#[test]
fn crates_missing_log_root_exits_one() {
    let output = run_ccturn(&[
        "crates",
        "--log-root",
        "/does/not/exist/ccturn-phase-2-step-7",
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "crates against a missing log root must exit 1; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// =========================================================================
// Phase 2 — `tracks` against the multi-session fixture.
// =========================================================================

#[test]
fn tracks_multi_session_default_renders_full_uuid_and_subagent_tree() {
    let output = run_ccturn(&["tracks", MULTI, "--log-root", "tests/fixtures/projects"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tracks default mode must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_str(&output);

    // Header: the project's encoded_cwd line.
    assert!(
        stdout.contains(&format!("Encoded   {MULTI}")),
        "default mode must print the encoded_cwd header; got:\n{stdout}"
    );

    // The success session UUID appears verbatim (no truncation).
    assert!(
        stdout.contains(&format!("session {SUCCESS_SID}")),
        "default mode must print the FULL session UUID; got:\n{stdout}"
    );

    // The status lines for each status appear at least once across the
    // visible sessions (unknown is omitted here — handled by the
    // chmod-000 test below).
    for status in ["success", "error", "aborted"] {
        assert!(
            stdout.contains(&format!("Status: {status}")),
            "default mode must surface `Status: {status}` line for at least one session; got:\n{stdout}"
        );
    }

    // The success session's subagent tree must render with box-drawing
    // characters and the meta.json descriptions.
    assert!(
        stdout.contains("Subagents (2):"),
        "default mode must label the success session's tree as `Subagents (2):`; got:\n{stdout}"
    );
    assert!(
        stdout.contains("├─") && stdout.contains("└─"),
        "default mode must use box-drawing chars for the subagent tree; got:\n{stdout}"
    );
    assert!(
        stdout.contains("Explore the project layout"),
        "default mode must render the Explore subagent's meta.json description; got:\n{stdout}"
    );
    assert!(
        stdout.contains("Plan the implementation"),
        "default mode must render the Plan subagent's meta.json description; got:\n{stdout}"
    );
}

#[test]
fn tracks_multi_session_default_renders_no_description_placeholder() {
    let output = run_ccturn(&["tracks", MULTI, "--log-root", "tests/fixtures/projects"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tracks default mode must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_str(&output);
    // The bbbbbbbb session has a single Task tool_use whose
    // agent-nodesc-id-3.meta.json sidecar is deliberately missing — the
    // continuation line must render the literal `(no description)`.
    assert!(
        stdout.contains("(no description)"),
        "default mode must render `(no description)` for the missing-meta subagent; got:\n{stdout}"
    );
}

#[test]
fn tracks_multi_session_json_has_subagents_and_status_values() {
    let output = run_ccturn(&[
        "tracks",
        MULTI,
        "--json",
        "--log-root",
        "tests/fixtures/projects",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tracks --json must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    let sessions = json["sessions"]
        .as_array()
        .expect("sessions must be a JSON array");
    assert!(
        sessions.len() >= 5,
        "multi-session fixture has 5 committed JSONLs; got {} rows",
        sessions.len()
    );

    let statuses: Vec<&str> = sessions
        .iter()
        .map(|s| s["status"].as_str().expect("status must be a string"))
        .collect();
    for expected in ["success", "error", "aborted"] {
        assert!(
            statuses.contains(&expected),
            "tracks --json must surface `{expected}` status; got {statuses:?}"
        );
    }

    // At least one row carries a non-empty subagents[] array (the
    // success session has two Tasks; the aborted session has one
    // dangling Task; the bbbbbbbb session has one Task).
    let any_with_subs = sessions.iter().any(|s| {
        s["subagents"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    });
    assert!(
        any_with_subs,
        "tracks --json must include at least one row with a non-empty subagents[] array; got {sessions:?}"
    );

    // The aborted session's dangling Task subagent must carry
    // status=aborted with null numerics.
    let aborted = sessions
        .iter()
        .find(|s| s["session_id"] == ABORTED_SID)
        .expect("aborted session must appear in sessions[]");
    let dangling_sub = aborted["subagents"]
        .as_array()
        .and_then(|a| a.first())
        .expect("aborted session must have one dangling Task subagent");
    assert_eq!(
        dangling_sub["status"], "aborted",
        "dangling Task subagent must carry status=aborted; got {dangling_sub}"
    );
    assert!(
        dangling_sub["total_duration_ms"].is_null(),
        "dangling Task subagent's total_duration_ms must be null; got {dangling_sub}"
    );
}

#[test]
fn tracks_multi_session_with_limit_caps_after_sort_and_reports_total() {
    let output = run_ccturn(&[
        "tracks",
        MULTI,
        "-n",
        "2",
        "--json",
        "--log-root",
        "tests/fixtures/projects",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tracks -n 2 --json must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    assert_eq!(
        json["session_count"], 5,
        "session_count must report the pre-truncation total (5 committed JSONLs)"
    );
    let sessions = json["sessions"].as_array().expect("sessions must be array");
    assert_eq!(
        sessions.len(),
        2,
        "-n 2 must truncate the sessions array to 2 rows; got {} rows",
        sessions.len()
    );
    // The two most recent sessions by started_at are SUCCESS_SID
    // (2026-05-22T18:00) and NODESC_SID (2026-05-22T17:00).
    let ids: Vec<&str> = sessions
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![SUCCESS_SID, NODESC_SID],
        "after sort+truncation the two newest sessions must remain in started_at desc order"
    );
}

#[test]
fn tracks_multi_session_oneline_eight_char_prefix_and_subagent_tag() {
    let output = run_ccturn(&[
        "tracks",
        MULTI,
        "--oneline",
        "--log-root",
        "tests/fixtures/projects",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tracks --oneline must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_str(&output);

    // Body lines (skip header).
    let body_lines: Vec<&str> = stdout
        .lines()
        .filter(|l| {
            !l.starts_with("Project")
                && !l.starts_with("Encoded")
                && !l.starts_with("Sessions")
                && !l.is_empty()
        })
        .collect();

    // Every body line must start with an 8-char hex prefix of the session
    // UUID (our fixture session IDs all start with hex runs).
    for line in &body_lines {
        let prefix: String = line.chars().take(8).collect();
        assert!(
            prefix.chars().all(|c| c.is_ascii_hexdigit()),
            "oneline body line must start with an 8-char hex prefix; got line: {line}"
        );
    }

    // At least one line ends with a `[N subagent...]` tag for sessions
    // that have subagents (the SUCCESS, NODESC, and ABORTED rows each
    // carry one).
    let with_tag: usize = body_lines.iter().filter(|l| l.contains("subagent")).count();
    assert!(
        with_tag >= 2,
        "at least the SUCCESS (2 subagents) and ABORTED (1 dangling Task) rows must carry `[N subagent...]` tags; got body:\n{}",
        body_lines.join("\n")
    );
}

#[test]
fn tracks_oneline_and_json_conflict_exits_sixty_four() {
    let output = run_ccturn(&[
        "tracks",
        MULTI,
        "--oneline",
        "--json",
        "--log-root",
        "tests/fixtures/projects",
    ]);
    assert_eq!(
        output.status.code(),
        Some(64),
        "tracks --oneline --json must exit 64 (clap conflict); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// =========================================================================
// Phase 2 — `tracks` against the empty / missing project cases.
// =========================================================================

#[test]
fn tracks_empty_project_yields_zero_sessions() {
    let output = run_ccturn(&[
        "tracks",
        EMPTY,
        "--json",
        "--log-root",
        "tests/fixtures/projects",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "tracks against an empty project must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    assert_eq!(
        json["session_count"], 0,
        "empty project must report session_count=0; got {json}"
    );
    let sessions = json["sessions"].as_array().expect("sessions must be array");
    assert!(
        sessions.is_empty(),
        "empty project must yield empty sessions[]; got {sessions:?}"
    );
}

#[test]
fn tracks_missing_project_exits_one() {
    let output = run_ccturn(&[
        "tracks",
        "-tmp-does-not-exist-step7",
        "--log-root",
        "tests/fixtures/projects",
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "tracks against a missing project must exit 1; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// =========================================================================
// Phase 2 — Unknown status via chmod 000 in a TempDir copy.
//
// The test copies the multi-session fixture into a tempdir, chmods the
// unknown jsonl to 000 within the copy, runs ccturn against the copy, and
// asserts the JSON contains a row with status=unknown. The original
// committed fixture is never mutated so the other integration tests run
// in parallel without races.
// =========================================================================

#[cfg(unix)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn tracks_multi_session_unknown_status_via_chmod_zero() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("tempdir");
    let dst_log_root = tmp.path().join("projects");
    let src_log_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/projects");
    copy_dir_recursive(&src_log_root, &dst_log_root).expect("copy fixture tree");

    let unknown_path = dst_log_root
        .join(MULTI)
        .join(format!("{UNKNOWN_SID}.jsonl"));
    fs::set_permissions(&unknown_path, fs::Permissions::from_mode(0o000))
        .expect("chmod 000 to simulate unreadable jsonl");

    let output = run_ccturn(&[
        "tracks",
        MULTI,
        "--json",
        "--log-root",
        dst_log_root
            .to_str()
            .expect("log root path must be valid UTF-8"),
    ]);

    // Restore permissions BEFORE asserting so TempDir can clean up even
    // if the assertions panic.
    let _ = fs::set_permissions(&unknown_path, fs::Permissions::from_mode(0o644));

    assert_eq!(
        output.status.code(),
        Some(0),
        "per-session failures must NOT propagate as a non-zero exit; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_str(&output))
        .expect("--json output must be a single JSON object");
    let unknown = json["sessions"]
        .as_array()
        .expect("sessions must be array")
        .iter()
        .find(|s| s["session_id"] == UNKNOWN_SID)
        .expect("unknown session row must be present");
    assert_eq!(
        unknown["status"], "unknown",
        "the chmod-000'd JSONL must surface as status=unknown; got {unknown}"
    );
}

// Suppress dead-code lint for the const that's only consumed by the
// chmod-test on Unix.
#[cfg(not(unix))]
#[allow(dead_code)]
const _UNKNOWN_SID_USED_ON_UNIX_ONLY: &str = UNKNOWN_SID;

// Suppress dead-code lint for ERROR_SID which is currently asserted only
// implicitly through the status-values test above; keep the const so
// future regressions on the error session are easy to pin.
#[allow(dead_code)]
const _ERROR_SID_KEPT_FOR_FUTURE_USE: &str = ERROR_SID;
