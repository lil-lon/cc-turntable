use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Serialize;
use walkdir::WalkDir;

// kebab-case renders `FirstRecord` -> "first-record" and
// `ReconstructedFromEncodedCwd` -> "reconstructed-from-encoded-cwd", matching
// § JSON Output Schema's `cwd_source` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CwdSource {
    FirstRecord,
    ReconstructedFromEncodedCwd,
}

#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub jsonl_path: PathBuf,
    pub subagents_dir: PathBuf,
    // Set on every resolve and asserted by the locator tests; the report path does
    // not currently surface it. Kept on the struct as the canonical encoded form
    // for future drill-down callers.
    #[allow(dead_code)]
    pub project_cwd_encoded: String,
    pub project_cwd: PathBuf,
    pub cwd_source: CwdSource,
}

pub fn default_log_root() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(value) if !value.is_empty() => PathBuf::from(value).join("projects"),
        _ => {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
            home.join(".claude").join("projects")
        }
    }
}

pub fn resolve(
    log_root: &Path,
    session_id: &str,
    project: Option<&str>,
) -> anyhow::Result<ResolvedSession> {
    if !log_root.exists() {
        return Err(anyhow!("log root {} does not exist", log_root.display()));
    }

    let file_name = format!("{session_id}.jsonl");

    let matches: Vec<PathBuf> = match project {
        Some(project_token) => {
            let candidate = log_root.join(project_token).join(&file_name);
            if candidate.is_file() {
                vec![candidate]
            } else {
                Vec::new()
            }
        }
        None => WalkDir::new(log_root)
            .min_depth(2)
            .max_depth(2)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name == file_name.as_str())
            })
            .collect(),
    };

    let jsonl_path = match matches.len() {
        0 => {
            return Err(anyhow!(
                "session id {} not found under {}",
                session_id,
                log_root.display()
            ));
        }
        1 => matches.into_iter().next().expect("non-empty"),
        _ => {
            let listing = matches
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(anyhow!(
                "session id {} matches multiple projects ({}); pass --project to disambiguate",
                session_id,
                listing
            ));
        }
    };

    let project_dir = jsonl_path
        .parent()
        .ok_or_else(|| anyhow!("jsonl path {} has no parent", jsonl_path.display()))?
        .to_path_buf();
    let project_cwd_encoded = project_dir
        .file_name()
        .ok_or_else(|| anyhow!("project dir {} has no name", project_dir.display()))?
        .to_string_lossy()
        .into_owned();
    let subagents_dir = project_dir.join(session_id).join("subagents");

    let (project_cwd, cwd_source) = match read_first_cwd_in_session(&jsonl_path) {
        Some(cwd) => (PathBuf::from(cwd), CwdSource::FirstRecord),
        None => (
            PathBuf::from(reconstruct_cwd_from_encoded(&project_cwd_encoded)),
            CwdSource::ReconstructedFromEncodedCwd,
        ),
    };

    Ok(ResolvedSession {
        jsonl_path,
        subagents_dir,
        project_cwd_encoded,
        project_cwd,
        cwd_source,
    })
}

// Scans the session JSONL for the first record that carries a top-level `cwd`
// field. Real Claude Code sessions often begin with a `file-history-snapshot`
// record that has no `cwd`, so reading the literal first line and giving up is
// too narrow — we'd fall back to the lossy encoded-cwd reconstruction even when
// the ground-truth path is sitting on line 2.
pub(crate) fn read_first_cwd_in_session(jsonl_path: &Path) -> Option<String> {
    let file = File::open(jsonl_path)
        .with_context(|| format!("open {}", jsonl_path.display()))
        .ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim_end()) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            return Some(cwd.to_owned());
        }
    }
    None
}

pub(crate) fn reconstruct_cwd_from_encoded(encoded: &str) -> String {
    encoded.replace('-', "/")
}

// Phase A test specification for the session locator (design doc § Implementation
// :: Step 3). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub fn default_log_root() -> PathBuf` — read `CLAUDE_CONFIG_DIR` from env
//     and append `/projects`; fall back to `<home>/.claude/projects` when the
//     env var is unset or empty.
//   - `pub fn resolve(log_root: &Path, session_id: &str, project: Option<&str>)
//      -> anyhow::Result<ResolvedSession>` — locate the session JSONL under the
//     log root, optionally scoped to one `--project` subdirectory.
//   - `pub struct ResolvedSession { jsonl_path, subagents_dir,
//      project_cwd_encoded, project_cwd, cwd_source }`.
//   - `pub enum CwdSource { FirstRecord, ReconstructedFromEncodedCwd }`.
//
// The tests below define the behavioural contract; they reference
// `super::{default_log_root, resolve, CwdSource}` and will compile once those
// items exist.

#[cfg(test)]
mod tests {
    use super::{CwdSource, default_log_root, resolve};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use tempfile::TempDir;

    // The two `default_log_root` tests both mutate process-global env state.
    // cargo runs tests in parallel by default, so we serialise the env-touching
    // ones with this module-private mutex (per Director's arbitration on
    // paragraph-Implementation :: Step 3). Mutex poisoning is recovered via
    // `unwrap_or_else(PoisonError::into_inner)` so a panicking sibling test
    // does not cascade.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // RAII guard that restores `CLAUDE_CONFIG_DIR` (or any env var) on drop,
    // even when the test panics. Edition-2024 makes env mutation `unsafe`,
    // so the calls are wrapped accordingly.
    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }

        fn unset(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match self.original.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    // Build a fake project tree under `log_root`:
    //   <log_root>/<encoded_cwd>/<session_id>.jsonl  ← contains `content`
    // Returns (jsonl_path, expected_subagents_dir). The subagents dir is
    // computed but not created — locator's contract is to return the path
    // whether or not the directory exists on disk.
    fn write_session_jsonl(
        log_root: &Path,
        encoded_cwd: &str,
        session_id: &str,
        content: &str,
    ) -> (PathBuf, PathBuf) {
        let project_dir = log_root.join(encoded_cwd);
        fs::create_dir_all(&project_dir).expect("create project dir");
        let jsonl_path = project_dir.join(format!("{session_id}.jsonl"));
        fs::write(&jsonl_path, content).expect("write session jsonl");
        let subagents_dir = project_dir.join(session_id).join("subagents");
        (jsonl_path, subagents_dir)
    }

    // ---------------------------------------------------------------------
    // Default-log-root resolution — `CLAUDE_CONFIG_DIR` env-var contract.
    // ---------------------------------------------------------------------

    #[test]
    fn default_log_root_respects_claude_config_dir_when_set() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = EnvVarGuard::set("CLAUDE_CONFIG_DIR", "/test/some/path");
        assert_eq!(
            default_log_root(),
            PathBuf::from("/test/some/path/projects"),
            "default_log_root must append `/projects` to $CLAUDE_CONFIG_DIR when set"
        );
    }

    #[test]
    fn default_log_root_falls_back_to_dot_claude_projects_when_unset() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = EnvVarGuard::unset("CLAUDE_CONFIG_DIR");
        let path = default_log_root();
        assert!(
            path.ends_with(".claude/projects"),
            "expected fallback to end with `.claude/projects`, got {}",
            path.display()
        );
    }

    // ---------------------------------------------------------------------
    // `resolve` — single-project happy path.
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_finds_session_in_single_project() {
        let tmp = TempDir::new().unwrap();
        let session_id = "session-aaaa";
        let (jsonl_path, expected_subagents_dir) = write_session_jsonl(
            tmp.path(),
            "-tmp-test-project",
            session_id,
            r#"{"type":"user","cwd":"/tmp/test-project","sessionId":"session-aaaa","timestamp":"2026-05-19T09:14:02.000000+00:00"}"#,
        );
        let resolved = resolve(tmp.path(), session_id, None)
            .expect("resolve must succeed when exactly one project contains the session");
        assert_eq!(resolved.jsonl_path, jsonl_path);
        assert_eq!(resolved.subagents_dir, expected_subagents_dir);
        assert_eq!(resolved.project_cwd_encoded, "-tmp-test-project");
    }

    // ---------------------------------------------------------------------
    // `resolve` — `--project` scoped lookup short-circuits the multi-project
    // walk and reads the named subdirectory directly.
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_with_project_flag_uses_scoped_lookup() {
        let tmp = TempDir::new().unwrap();
        let session_id = "session-bbbb";
        // Two projects, both contain the same session id — without `--project`
        // this would be ambiguous; the scoped lookup must pick the named one
        // and NOT raise the ambiguity error.
        let (jsonl_a, expected_subagents_a) = write_session_jsonl(
            tmp.path(),
            "-tmp-project-a",
            session_id,
            r#"{"type":"user","cwd":"/tmp/project-a","sessionId":"session-bbbb","timestamp":"2026-05-19T09:14:02.000000+00:00"}"#,
        );
        let (_jsonl_b, _expected_subagents_b) = write_session_jsonl(
            tmp.path(),
            "-tmp-project-b",
            session_id,
            r#"{"type":"user","cwd":"/tmp/project-b","sessionId":"session-bbbb","timestamp":"2026-05-19T09:14:02.000000+00:00"}"#,
        );
        let resolved = resolve(tmp.path(), session_id, Some("-tmp-project-a"))
            .expect("--project scoped resolve must succeed despite the other project's session");
        assert_eq!(resolved.jsonl_path, jsonl_a);
        assert_eq!(resolved.subagents_dir, expected_subagents_a);
        assert_eq!(resolved.project_cwd_encoded, "-tmp-project-a");
    }

    // ---------------------------------------------------------------------
    // `resolve` — when `project` is None and two projects contain the same
    // session id, return Err (the CLI surfaces this with a "pass --project"
    // hint, but the lib-level contract is just Err).
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_returns_error_on_multi_project_ambiguity() {
        let tmp = TempDir::new().unwrap();
        let session_id = "session-cccc";
        let _ = write_session_jsonl(
            tmp.path(),
            "-tmp-project-x",
            session_id,
            r#"{"type":"user","cwd":"/tmp/project-x","sessionId":"session-cccc","timestamp":"2026-05-19T09:14:02.000000+00:00"}"#,
        );
        let _ = write_session_jsonl(
            tmp.path(),
            "-tmp-project-y",
            session_id,
            r#"{"type":"user","cwd":"/tmp/project-y","sessionId":"session-cccc","timestamp":"2026-05-19T09:14:02.000000+00:00"}"#,
        );
        let result = resolve(tmp.path(), session_id, None);
        assert!(
            result.is_err(),
            "multi-project ambiguity must return Err; got Ok"
        );
    }

    // ---------------------------------------------------------------------
    // `resolve` — missing log root is Err (distinct from "session not
    // found"; surfaced by the CLI as "error: log root <path> does not exist").
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_returns_error_when_log_root_missing() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let result = resolve(&missing, "session-zzzz", None);
        assert!(
            result.is_err(),
            "missing log root must return Err; got Ok with {:?}",
            result.ok().map(|r| r.jsonl_path)
        );
    }

    // ---------------------------------------------------------------------
    // `resolve` — `project_cwd` is the ground-truth `cwd` field from the
    // first record of the session JSONL, NOT the reconstructed-from-
    // encoded-cwd value. The test pins this by deliberately making the
    // encoded-cwd directory name reconstruct to a DIFFERENT path than the
    // JSONL's `cwd`.
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_reads_ground_truth_cwd_from_first_record() {
        let tmp = TempDir::new().unwrap();
        let session_id = "session-dddd";
        let encoded = "-tmp-encoded-form";
        let reconstructed_if_lossy = "/tmp/encoded/form";
        let ground_truth = "/tmp/actual-cwd-from-file";
        let _ = write_session_jsonl(
            tmp.path(),
            encoded,
            session_id,
            &format!(
                r#"{{"type":"user","cwd":"{ground_truth}","sessionId":"{session_id}","timestamp":"2026-05-19T09:14:02.000000+00:00"}}"#,
            ),
        );
        let resolved = resolve(tmp.path(), session_id, None).expect("resolve must succeed");
        assert_eq!(
            resolved.project_cwd,
            PathBuf::from(ground_truth),
            "project_cwd must be the verbatim `cwd` from the first record"
        );
        assert_ne!(
            resolved.project_cwd,
            PathBuf::from(reconstructed_if_lossy),
            "project_cwd must NOT be the encoded-cwd reconstruction when the JSONL is readable"
        );
        assert!(
            matches!(resolved.cwd_source, CwdSource::FirstRecord),
            "cwd_source must be FirstRecord when the JSONL has a readable `cwd` on the first record"
        );
        assert_eq!(resolved.project_cwd_encoded, encoded);
    }

    // ---------------------------------------------------------------------
    // `resolve` — real Claude Code sessions begin with a `file-history-
    // snapshot` record that has no top-level `cwd`. The ground truth lives
    // on a later record (typically the first `user` line). The locator
    // must scan forward, NOT give up after the literal first line and
    // fall back to the lossy encoded-cwd reconstruction.
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_reads_ground_truth_cwd_from_first_record_that_carries_cwd() {
        let tmp = TempDir::new().unwrap();
        let session_id = "session-ffff";
        // The encoded-cwd token contains a `-` inside one of the directory
        // names (`lil-lon`). If the locator falls back to reconstruction it
        // turns those internal hyphens into path separators, scrambling the
        // path. The test pins that fallback is NOT taken when the cwd is
        // discoverable elsewhere in the file.
        let encoded = "-Users-me-lil-lon-repo";
        let ground_truth = "/Users/me/lil-lon/repo";
        let snapshot_line = r#"{"type":"file-history-snapshot","messageId":"sn1","snapshot":{"messageId":"sn1","trackedFileBackups":{},"timestamp":"2026-05-19T09:00:00.000Z"},"isSnapshotUpdate":false}"#;
        let user_line = format!(
            r#"{{"type":"user","cwd":"{ground_truth}","sessionId":"{session_id}","timestamp":"2026-05-19T09:00:01.000Z","uuid":"u1"}}"#,
        );
        let body = format!("{snapshot_line}\n{user_line}\n");
        let _ = write_session_jsonl(tmp.path(), encoded, session_id, &body);
        let resolved = resolve(tmp.path(), session_id, None).expect("resolve must succeed");
        assert_eq!(
            resolved.project_cwd,
            PathBuf::from(ground_truth),
            "project_cwd must be the `cwd` from the FIRST record that carries one, \
             not the lossy reconstruction of the encoded-cwd directory name"
        );
        assert!(
            matches!(resolved.cwd_source, CwdSource::FirstRecord),
            "cwd_source is FirstRecord whenever the ground truth was read from the JSONL, \
             regardless of whether the cwd-carrying record was on line 1 or later"
        );
    }

    // ---------------------------------------------------------------------
    // `resolve` — empty session JSONL triggers the reconstructed-from-
    // encoded-cwd fallback, with `cwd_source = ReconstructedFromEncodedCwd`
    // so downstream code can warn about lossy paths.
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_reconstructs_cwd_from_encoded_when_jsonl_empty() {
        let tmp = TempDir::new().unwrap();
        let session_id = "session-eeee";
        // `-tmp-foo-bar` reconstructs unambiguously to `/tmp/foo/bar`
        // (no hyphens in the original path => no ambiguity).
        let encoded = "-tmp-foo-bar";
        let reconstructed = "/tmp/foo/bar";
        let _ = write_session_jsonl(tmp.path(), encoded, session_id, "");
        let resolved = resolve(tmp.path(), session_id, None).expect("resolve must succeed");
        assert_eq!(
            resolved.project_cwd,
            PathBuf::from(reconstructed),
            "empty JSONL must trigger the reconstructed-from-encoded-cwd fallback"
        );
        assert!(
            matches!(resolved.cwd_source, CwdSource::ReconstructedFromEncodedCwd),
            "cwd_source must be ReconstructedFromEncodedCwd when the JSONL is empty"
        );
        assert_eq!(resolved.project_cwd_encoded, encoded);
    }
}
