// Phase A integration test for the `ccturn spin` CLI (design doc § Implementation
// :: Step 12, tasks 3-4). This is a binary-crate integration test: it invokes the
// compiled `ccturn` binary as a subprocess (located via the `CARGO_BIN_EXE_ccturn`
// env var that cargo injects for integration tests) and asserts on its stdout and
// exit code.
//
// The full project-tree fixture lives under
// `tests/fixtures/projects/-tmp-test-project/`: the `integration-session.jsonl`
// session log plus the sibling `integration-session/subagents/` directory holding
// the agent log and `agent-<id>.meta.json`. The session JSONL exercises one Skill
// invocation, one Task subagent, one error of each of the four categories, and a
// mid-stream user intervention.
//
// NOTE for the Programmer (Step 12 Phase B): main.rs must write the human report
// to stdout with `print!` (NOT `println!`). The formatted report already ends
// with a newline; `println!` would append a spurious trailing blank line and
// break the byte-for-byte snapshot assertion below.

use std::process::{Command, Output};

// Runs `ccturn spin integration-session --log-root tests/fixtures/projects`
// (plus any extra args) with the working directory pinned to the crate root, so
// the relative `--log-root` and the relative paths in the report are stable.
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

// (a) Exact-string snapshot of the human-readable output, and exit code 0.
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

// (b) `--json` output parses as a single JSON object and satisfies the
// structural assertions from design doc Step 12 task 4; exit code 0.
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

    // project_cwd is the ground-truth `cwd` field from the first record of the
    // session JSONL — `/tmp/test-project` — NOT the lossy reconstruction of the
    // encoded-cwd directory name `-tmp-test-project`.
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
