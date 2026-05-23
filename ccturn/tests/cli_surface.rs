// Phase 2 Step 1 — CLI surface extension (`crates` / `tracks` subcommands).
//
// These tests verify the parse-time surface and the exit-code contract listed
// in the Step 1 task list:
//
//   * `Crates { json, log_root }` and `Tracks { project, limit, oneline, json,
//     log_root }` subcommands accepted by clap.
//   * `tracks` accepts both `-n N` and `--limit N` (clap's `short = 'n', long
//     = "limit"`).
//   * `--oneline` and `--json` are mutually exclusive (`conflicts_with`) and the
//     CLI rejects the combination at parse time with exit code 64.
//   * The Phase 1 exit-code contract holds for both new subcommands:
//     - log-root-missing -> 1
//     - clap usage error -> 64
//     - --help / --version -> 0
//
// Tests that exercise actual enumeration output (success-path assertions) live
// in `tests/integration.rs` and arrive with Step 7.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ccturn"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to spawn the ccturn binary")
}

// Run with an extra `--log-root <log_root>` appended. The directory must exist
// so the test isolates the flag-parsing concern from the log-root-missing
// branch (which is covered separately below).
fn run_with_log_root(args: &[&str], log_root: &Path) -> Output {
    let mut full: Vec<&str> = args.to_vec();
    full.push("--log-root");
    full.push(
        log_root
            .to_str()
            .expect("test temp path must be valid UTF-8"),
    );
    run(&full)
}

// Path used by the log-root-missing tests. Stable, unique to this test file,
// and deliberately under a parent that does not exist — so the check cannot be
// satisfied by an accidentally pre-existing directory.
const MISSING_LOG_ROOT: &str = "/does/not/exist/ccturn-phase2-cli-surface-tests";

// ---- `crates` parse surface ---------------------------------------------

#[test]
fn crates_subcommand_help_exits_zero() {
    let output = run(&["crates", "--help"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "`crates --help` must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn crates_missing_log_root_exits_one() {
    let output = run(&["crates", "--log-root", MISSING_LOG_ROOT]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "`crates --log-root <missing>` must exit 1 per Phase 1 contract; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- `tracks` parse surface ---------------------------------------------

#[test]
fn tracks_subcommand_help_exits_zero() {
    let output = run(&["tracks", "--help"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "`tracks --help` must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tracks_missing_positional_project_exits_sixty_four() {
    // `tracks` with no PROJECT argument is a clap usage error.
    let output = run(&["tracks"]);
    assert_eq!(
        output.status.code(),
        Some(64),
        "`tracks` (no PROJECT) must exit 64 (clap usage); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tracks_missing_log_root_exits_one() {
    let output = run(&[
        "tracks",
        "any-project-token",
        "--log-root",
        MISSING_LOG_ROOT,
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "`tracks PROJECT --log-root <missing>` must exit 1 per Phase 1 contract; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tracks_accepts_short_n_flag() {
    // `-n 5` is the git-log-style short form of `--limit 5`. The test asserts
    // clap parses it (i.e. the run does NOT exit 64). The eventual exit code
    // here depends on later steps; pre-Step-3 it may surface as 1 (project not
    // found) or 0 (empty), both of which are acceptable for this assertion.
    let tmp = TempDir::new().expect("tempdir creation must succeed");
    let output = run_with_log_root(&["tracks", "any-project-token", "-n", "5"], tmp.path());
    assert_ne!(
        output.status.code(),
        Some(64),
        "`tracks PROJECT -n 5` must parse (not be a clap usage error); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tracks_accepts_long_limit_flag() {
    let tmp = TempDir::new().expect("tempdir creation must succeed");
    let output = run_with_log_root(&["tracks", "any-project-token", "--limit", "5"], tmp.path());
    assert_ne!(
        output.status.code(),
        Some(64),
        "`tracks PROJECT --limit 5` must parse (not be a clap usage error); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tracks_accepts_oneline_flag() {
    let tmp = TempDir::new().expect("tempdir creation must succeed");
    let output = run_with_log_root(&["tracks", "any-project-token", "--oneline"], tmp.path());
    assert_ne!(
        output.status.code(),
        Some(64),
        "`tracks PROJECT --oneline` must parse (not be a clap usage error); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tracks_accepts_json_flag() {
    let tmp = TempDir::new().expect("tempdir creation must succeed");
    let output = run_with_log_root(&["tracks", "any-project-token", "--json"], tmp.path());
    assert_ne!(
        output.status.code(),
        Some(64),
        "`tracks PROJECT --json` must parse (not be a clap usage error); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- `--oneline` / `--json` conflict -----------------------------------

#[test]
fn tracks_oneline_and_json_conflict_exits_sixty_four() {
    // The design doc § CLI Surface mandates that `--oneline` is incompatible
    // with `--json`, enforced via clap's `conflicts_with` so the CLI rejects
    // the combination at parse time with exit code 64.
    let tmp = TempDir::new().expect("tempdir creation must succeed");
    let output = run_with_log_root(
        &["tracks", "any-project-token", "--oneline", "--json"],
        tmp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(64),
        "`tracks PROJECT --oneline --json` must exit 64 (clap conflict); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("oneline") && stderr.contains("json"),
        "clap conflict diagnostic must mention both '--oneline' and '--json'; got:\n{stderr}"
    );
}

#[test]
fn tracks_oneline_and_json_conflict_order_independent() {
    // Same as above but with the flags in the opposite order on the CLI, to
    // catch implementations that only mark conflicts_with in one direction.
    let tmp = TempDir::new().expect("tempdir creation must succeed");
    let output = run_with_log_root(
        &["tracks", "any-project-token", "--json", "--oneline"],
        tmp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(64),
        "`tracks PROJECT --json --oneline` must exit 64 (clap conflict, order-independent); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- Top-level help advertises the new subcommands ---------------------

#[test]
fn root_help_lists_crates_and_tracks() {
    let output = run(&["--help"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "`ccturn --help` must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("crates"),
        "`ccturn --help` must list the `crates` subcommand; got:\n{stdout}"
    );
    assert!(
        stdout.contains("tracks"),
        "`ccturn --help` must list the `tracks` subcommand; got:\n{stdout}"
    );
}
