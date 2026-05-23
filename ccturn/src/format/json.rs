use crate::report::SessionReport;

// `serde_json::to_string` is compact by default (no newlines, no padding).
// Timestamps round-trip verbatim because `SessionReport` stores the source
// strings unmodified; empty `Vec`s serialise to `[]`, never omitted.
pub fn format_json(report: &SessionReport) -> String {
    serde_json::to_string(report).expect("SessionReport must serialise to JSON")
}

// Phase A test specification for the JSON formatter (design doc § Implementation
// :: Step 11 and § JSON Output Schema). The Programmer adds the production code
// ABOVE this `#[cfg(test)]` block in Phase B:
//   - `pub fn format_json(report: &SessionReport) -> String` — serialise the
//     report to a single compact (non-pretty) JSON object. main.rs writes the
//     returned String to stdout for `--json`.
//
// Per § Timestamp formatting policy, JSON output preserves every source
// timestamp VERBATIM (microsecond precision, `+00:00` suffix) — the opposite
// of human output, which truncates to second precision with a `Z` suffix.
// Per § JSON Output Schema, empty arrays are emitted (never omitted) so
// consumers can index them unconditionally.

#[cfg(test)]
mod tests {
    use super::format_json;
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

    // ---------------------------------------------------------------------
    // The output is a single valid JSON object — it round-trips through
    // `serde_json::from_str` into a `serde_json::Value` that is an object.
    // ---------------------------------------------------------------------

    #[test]
    fn output_is_a_single_valid_json_object() {
        let report = report_from_fixture("full-session.jsonl");
        let json_str = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .unwrap_or_else(|e| panic!("format_json output must be valid JSON: {e}\n{json_str}"));
        assert!(
            parsed.is_object(),
            "format_json must emit a single top-level JSON object"
        );
    }

    // ---------------------------------------------------------------------
    // The output is compact (non-pretty): no newlines. `--json` consumers
    // pipe through `jq` if they want indentation.
    // ---------------------------------------------------------------------

    #[test]
    fn output_is_compact_not_pretty() {
        let report = report_from_fixture("full-session.jsonl");
        let json_str = format_json(&report);
        assert!(
            !json_str.contains('\n'),
            "compact JSON output must contain no newlines"
        );
    }

    // ---------------------------------------------------------------------
    // All twelve top-level keys from § JSON Output Schema are present, and
    // there are exactly twelve (no extras).
    // ---------------------------------------------------------------------

    #[test]
    fn all_twelve_top_level_schema_keys_are_present() {
        let report = report_from_fixture("full-session.jsonl");
        let json_str = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let obj = parsed
            .as_object()
            .expect("the output must be a JSON object");

        for key in [
            "session_id",
            "project_cwd",
            "cwd_source",
            "log_path",
            "started_at",
            "ended_at",
            "record_count",
            "skills",
            "subagents",
            "tools",
            "errors",
            "interventions",
        ] {
            assert!(obj.contains_key(key), "missing top-level key `{key}`");
        }
        assert_eq!(
            obj.len(),
            12,
            "exactly twelve top-level keys; got {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    // ---------------------------------------------------------------------
    // Timestamp formatting policy (JSON side) — timestamps are preserved
    // VERBATIM with microsecond precision and the `+00:00` offset, in
    // contrast to human output which truncates to a `Z` suffix.
    // ---------------------------------------------------------------------

    #[test]
    fn timestamps_are_preserved_verbatim_with_microsecond_precision() {
        let report = report_from_fixture("full-session.jsonl");
        let json_str = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(
            parsed["started_at"], "2026-05-19T09:00:00.000000+00:00",
            "started_at must keep the verbatim microsecond `+00:00` form"
        );
        assert_eq!(
            parsed["ended_at"], "2026-05-19T09:00:18.000000+00:00",
            "ended_at must keep the verbatim microsecond `+00:00` form"
        );
        assert!(
            json_str.contains(".000000+00:00"),
            "JSON output must NOT truncate timestamps to second precision"
        );
    }

    // ---------------------------------------------------------------------
    // Empty arrays are emitted, never omitted — the `empty-sections.jsonl`
    // fixture yields a report whose five extractor arrays are all empty;
    // every array key must still be present with a `[]` value so consumers
    // can index them unconditionally.
    // ---------------------------------------------------------------------

    #[test]
    fn empty_arrays_are_emitted_not_omitted() {
        let report = report_from_fixture("empty-sections.jsonl");
        let json_str = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        for key in ["skills", "subagents", "tools", "errors", "interventions"] {
            let value = parsed
                .get(key)
                .unwrap_or_else(|| panic!("array key `{key}` must be present, never omitted"));
            let arr = value
                .as_array()
                .unwrap_or_else(|| panic!("`{key}` must serialise as a JSON array"));
            assert!(
                arr.is_empty(),
                "`{key}` must be an empty array for the empty-sections fixture"
            );
        }
    }
}
