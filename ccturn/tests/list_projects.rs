// Phase 2 Step 2 — Project enumeration module (`src/list/projects.rs`).
//
// These integration tests exercise the public entry point
//   pub fn list_projects(log_root: &Path) -> anyhow::Result<ProjectListing>
// directly. The two Phase 1 helpers it delegates to
// (`read_first_cwd_in_session`, `reconstruct_cwd_from_encoded`) are
// pub(crate) and are tested only indirectly through `list_projects`.
//
// This file assumes `ccturn` exposes a `lib.rs` library target so an
// integration test can `use ccturn::...`. The library target arrives with
// the Step 2 implementation; before then this file fails to compile, which
// is the expected TDD red phase.

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

use ccturn::list::projects::list_projects;
use ccturn::locator::CwdSource;

// ---- Fixture helpers ----------------------------------------------------

// Build `<log_root>/<encoded_cwd>/` and write a session JSONL named
// `<session_id>.jsonl` with the given body. Returns the JSONL path so the
// caller can stamp its mtime if needed.
fn write_session(log_root: &Path, encoded_cwd: &str, session_id: &str, body: &str) -> PathBuf {
    let project_dir = log_root.join(encoded_cwd);
    fs::create_dir_all(&project_dir).expect("create project dir");
    let jsonl = project_dir.join(format!("{session_id}.jsonl"));
    fs::write(&jsonl, body).expect("write session jsonl");
    jsonl
}

// Create an empty project directory under `log_root` with no JSONL children.
fn make_empty_project(log_root: &Path, encoded_cwd: &str) -> PathBuf {
    let p = log_root.join(encoded_cwd);
    fs::create_dir_all(&p).expect("create empty project dir");
    p
}

// Anchor for synthetic mtimes. ~2024-01-01 UTC. Offsets in seconds keep the
// arithmetic clearly ordered (large offset = newer file).
const MTIME_EPOCH: u64 = 1_700_000_000;

fn fixed_mtime(offset_secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(MTIME_EPOCH + offset_secs)
}

// Stamp `path`'s modification time to the given `SystemTime`.
fn set_mtime(path: &Path, mtime: SystemTime) {
    let file = File::options()
        .write(true)
        .open(path)
        .expect("open file to set mtime");
    let times = FileTimes::new().set_modified(mtime);
    file.set_times(times).expect("set mtime");
}

// One-record JSONL body carrying a top-level `cwd` field. Sufficient to
// drive the FirstRecord branch of project_cwd resolution.
fn jsonl_with_cwd(session_id: &str, cwd: &str) -> String {
    format!(
        r#"{{"type":"user","cwd":"{cwd}","sessionId":"{session_id}","timestamp":"2026-05-19T09:00:00.000Z","uuid":"u1"}}
"#,
    )
}

// One-record JSONL body that intentionally lacks a `cwd` field. Drives the
// reconstruction fallback for project_cwd.
fn jsonl_without_cwd(session_id: &str) -> String {
    format!(
        r#"{{"type":"file-history-snapshot","messageId":"{session_id}","snapshot":{{"messageId":"{session_id}","trackedFileBackups":{{}},"timestamp":"2026-05-19T09:00:00.000Z"}},"isSnapshotUpdate":false}}
"#,
    )
}

// Locate a row by its encoded_cwd. Panics on miss — the call sites all set
// up known fixtures so a miss is a real failure, not a test-data quirk.
fn row_by_encoded<'a>(
    listing: &'a ccturn::list::projects::ProjectListing,
    encoded: &str,
) -> &'a ccturn::list::projects::ProjectRow {
    listing
        .projects
        .iter()
        .find(|p| p.encoded_cwd == encoded)
        .unwrap_or_else(|| {
            panic!(
                "no row with encoded_cwd `{encoded}`; got {:?}",
                listing
                    .projects
                    .iter()
                    .map(|p| &p.encoded_cwd)
                    .collect::<Vec<_>>()
            )
        })
}

// ---- Log-root handling --------------------------------------------------

#[test]
fn list_projects_returns_err_when_log_root_missing() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("does-not-exist");
    let result = list_projects(&missing);
    assert!(
        result.is_err(),
        "missing log root must return Err; got Ok({:?})",
        result.ok().map(|l| l.projects.len())
    );
}

#[test]
fn list_projects_empty_log_root_yields_empty_listing_with_log_root_set() {
    let tmp = TempDir::new().unwrap();
    let listing = list_projects(tmp.path()).expect("empty log root must succeed");
    assert!(
        listing.projects.is_empty(),
        "empty log root must yield zero project rows; got {} rows",
        listing.projects.len()
    );
    assert_eq!(
        listing.log_root,
        tmp.path(),
        "listing.log_root must echo the input path verbatim"
    );
}

#[test]
fn list_projects_skips_non_directory_children_at_log_root() {
    let tmp = TempDir::new().unwrap();
    // One real project dir, plus a stray file directly under the log root.
    write_session(
        tmp.path(),
        "-tmp-real-project",
        "session-a",
        &jsonl_with_cwd("session-a", "/tmp/real-project"),
    );
    fs::write(tmp.path().join("stray-file.txt"), "noise").expect("write stray file");

    let listing = list_projects(tmp.path()).expect("listing must succeed");
    assert_eq!(
        listing.projects.len(),
        1,
        "non-directory children at log root must be skipped; got {} rows",
        listing.projects.len()
    );
    assert_eq!(listing.projects[0].encoded_cwd, "-tmp-real-project");
}

// ---- Session count rules ------------------------------------------------

#[test]
fn list_projects_counts_only_jsonl_files() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-tmp-mixed");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("session-1.jsonl"),
        jsonl_without_cwd("session-1"),
    )
    .unwrap();
    fs::write(project.join("notes.txt"), "scratch").unwrap();
    fs::write(project.join("README.md"), "readme").unwrap();
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, "-tmp-mixed");
    assert_eq!(
        row.session_count, 1,
        "session_count must only count `*.jsonl` files; got {}",
        row.session_count
    );
}

#[test]
fn list_projects_does_not_count_subagent_sibling_directory_as_session() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-tmp-with-subagents");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("session-1.jsonl"),
        jsonl_without_cwd("session-1"),
    )
    .unwrap();
    // The Phase 1 layout puts a sibling directory at <session-id>/ next to
    // the JSONL. It has no `.jsonl` suffix and must NOT be counted as a
    // session — even though directory enumeration would otherwise see it.
    fs::create_dir_all(project.join("session-1").join("subagents")).unwrap();
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, "-tmp-with-subagents");
    assert_eq!(
        row.session_count, 1,
        "subagent sibling directory must not count as a session; got {}",
        row.session_count
    );
}

#[test]
fn list_projects_session_count_is_zero_for_empty_project_dir() {
    let tmp = TempDir::new().unwrap();
    make_empty_project(tmp.path(), "-tmp-empty");
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, "-tmp-empty");
    assert_eq!(row.session_count, 0);
}

// ---- encoded_cwd verbatim ----------------------------------------------

#[test]
fn list_projects_encoded_cwd_is_directory_name_verbatim() {
    let tmp = TempDir::new().unwrap();
    let encoded = "-Users-me-lil-lon-repo";
    write_session(
        tmp.path(),
        encoded,
        "session-x",
        &jsonl_with_cwd("session-x", "/Users/me/lil-lon/repo"),
    );
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, encoded);
    assert_eq!(
        row.encoded_cwd, encoded,
        "encoded_cwd must be the directory name verbatim — no decoding"
    );
}

// ---- latest_session_at -------------------------------------------------

#[test]
fn list_projects_latest_session_at_is_none_when_project_has_no_jsonl() {
    let tmp = TempDir::new().unwrap();
    make_empty_project(tmp.path(), "-tmp-empty");
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, "-tmp-empty");
    assert!(
        row.latest_session_at.is_none(),
        "latest_session_at must be None for a project with no JSONL children; got {:?}",
        row.latest_session_at
    );
}

#[test]
fn list_projects_latest_session_at_picks_max_mtime_across_jsonl_children() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("-tmp-multi");
    fs::create_dir_all(&project).unwrap();

    // Three sessions; we stamp explicit mtimes so the assertion is decoupled
    // from filesystem write-time ordering.
    let a = project.join("session-a.jsonl");
    let b = project.join("session-b.jsonl");
    let c = project.join("session-c.jsonl");
    fs::write(&a, jsonl_without_cwd("session-a")).unwrap();
    fs::write(&b, jsonl_without_cwd("session-b")).unwrap();
    fs::write(&c, jsonl_without_cwd("session-c")).unwrap();
    let oldest = fixed_mtime(0);
    let middle = fixed_mtime(60);
    let newest = fixed_mtime(120);
    set_mtime(&a, oldest);
    set_mtime(&b, newest);
    set_mtime(&c, middle);

    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, "-tmp-multi");
    let latest = row
        .latest_session_at
        .expect("latest_session_at must be Some when at least one JSONL exists");

    // `latest_session_at` is exposed as a chrono::DateTime<Utc>. Compare via
    // its SystemTime conversion so the assertion does not depend on the
    // serialization-side string format.
    let observed: SystemTime = latest.into();
    let observed_secs = observed
        .duration_since(UNIX_EPOCH)
        .expect("observed mtime must be >= UNIX_EPOCH")
        .as_secs();
    let expected_secs = newest
        .duration_since(UNIX_EPOCH)
        .expect("expected mtime must be >= UNIX_EPOCH")
        .as_secs();
    assert_eq!(
        observed_secs, expected_secs,
        "latest_session_at must equal the max mtime of the JSONL children"
    );
}

// ---- project_cwd / cwd_source -----------------------------------------

#[test]
fn list_projects_project_cwd_is_first_record_when_recent_jsonl_has_cwd() {
    let tmp = TempDir::new().unwrap();
    let encoded = "-tmp-encoded-form";
    let ground_truth = "/tmp/actual-cwd-from-file";
    write_session(
        tmp.path(),
        encoded,
        "session-d",
        &jsonl_with_cwd("session-d", ground_truth),
    );
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, encoded);
    assert_eq!(
        row.project_cwd.as_deref(),
        Some(Path::new(ground_truth)),
        "project_cwd must be the verbatim `cwd` from the most recent session's JSONL"
    );
    assert!(
        matches!(row.cwd_source, CwdSource::FirstRecord),
        "cwd_source must be FirstRecord when project_cwd was read from a JSONL"
    );
}

#[test]
fn list_projects_project_cwd_falls_back_to_reconstruction_when_no_jsonl_has_cwd() {
    let tmp = TempDir::new().unwrap();
    let encoded = "-tmp-foo-bar";
    let reconstructed = "/tmp/foo/bar";
    // The body is well-formed but has no `cwd` field anywhere — drives the
    // reconstruction fallback while keeping the file readable.
    write_session(
        tmp.path(),
        encoded,
        "session-e",
        &jsonl_without_cwd("session-e"),
    );
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, encoded);
    assert_eq!(
        row.project_cwd.as_deref(),
        Some(Path::new(reconstructed)),
        "project_cwd must fall back to the encoded-cwd reconstruction when no record carries `cwd`"
    );
    assert!(
        matches!(row.cwd_source, CwdSource::ReconstructedFromEncodedCwd),
        "cwd_source must be ReconstructedFromEncodedCwd when project_cwd was reconstructed"
    );
}

#[test]
fn list_projects_project_cwd_reconstructed_when_project_dir_has_no_jsonl() {
    let tmp = TempDir::new().unwrap();
    let encoded = "-tmp-empty-project";
    let reconstructed = "/tmp/empty/project";
    make_empty_project(tmp.path(), encoded);
    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, encoded);
    assert_eq!(
        row.project_cwd.as_deref(),
        Some(Path::new(reconstructed)),
        "project_cwd must be the encoded-cwd reconstruction when the project has no JSONL"
    );
    assert!(
        matches!(row.cwd_source, CwdSource::ReconstructedFromEncodedCwd),
        "cwd_source must be ReconstructedFromEncodedCwd for an empty project dir"
    );
}

#[test]
fn list_projects_project_cwd_reads_from_most_recent_jsonl() {
    let tmp = TempDir::new().unwrap();
    let encoded = "-tmp-two-sessions";
    let project = tmp.path().join(encoded);
    fs::create_dir_all(&project).unwrap();

    // Older session carries cwd = /old/path; newer carries /new/path. The
    // doc says the cwd lookup must use the most-recent JSONL.
    let older = project.join("session-older.jsonl");
    let newer = project.join("session-newer.jsonl");
    fs::write(&older, jsonl_with_cwd("session-older", "/old/path")).unwrap();
    fs::write(&newer, jsonl_with_cwd("session-newer", "/new/path")).unwrap();
    set_mtime(&older, fixed_mtime(0));
    set_mtime(&newer, fixed_mtime(120));

    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let row = row_by_encoded(&listing, encoded);
    assert_eq!(
        row.project_cwd.as_deref(),
        Some(Path::new("/new/path")),
        "project_cwd must come from the cwd of the MOST RECENT session JSONL"
    );
    assert!(matches!(row.cwd_source, CwdSource::FirstRecord));
}

// ---- Sort order --------------------------------------------------------

#[test]
fn list_projects_sorts_by_latest_desc_then_encoded_asc_with_nulls_last() {
    let tmp = TempDir::new().unwrap();

    // Project A: newer mtime
    let a_path = write_session(tmp.path(), "-tmp-aaa", "sa", &jsonl_without_cwd("sa"));
    set_mtime(&a_path, fixed_mtime(120));

    // Project B: older mtime
    let b_path = write_session(tmp.path(), "-tmp-bbb", "sb", &jsonl_without_cwd("sb"));
    set_mtime(&b_path, fixed_mtime(60));

    // Two empty projects (latest_session_at = None). The doc says they sort
    // last with encoded_cwd asc tie-break — so "-tmp-empty-aaa" comes before
    // "-tmp-empty-bbb".
    make_empty_project(tmp.path(), "-tmp-empty-bbb");
    make_empty_project(tmp.path(), "-tmp-empty-aaa");

    let listing = list_projects(tmp.path()).expect("listing must succeed");
    let order: Vec<&str> = listing
        .projects
        .iter()
        .map(|r| r.encoded_cwd.as_str())
        .collect();
    assert_eq!(
        order,
        vec!["-tmp-aaa", "-tmp-bbb", "-tmp-empty-aaa", "-tmp-empty-bbb"],
        "sort key must be (latest_session_at desc, nulls last, encoded_cwd asc)"
    );
}
