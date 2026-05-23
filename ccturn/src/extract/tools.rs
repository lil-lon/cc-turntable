use std::collections::HashMap;

use serde::Serialize;

use crate::extract::assistant_content_blocks;
use crate::parser::record::{ContentBlock, JsonlRecord};

#[derive(Debug, Clone, Serialize)]
pub struct ToolUsage {
    pub tool_name: String,
    pub invocation_count: usize,
    pub error_count: usize,
}

pub fn extract_tools(records: &[JsonlRecord]) -> Vec<ToolUsage> {
    // tool_use_id -> is_error of the matching tool_result.
    let mut error_map: HashMap<String, bool> = HashMap::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            if let ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } = block
            {
                error_map.insert(tool_use_id, is_error);
            }
        }
    }

    // tool_name -> (invocation_count, error_count). `Skill` and `Task` are
    // themselves `tool_use` blocks, so they accumulate their own rows here.
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                let entry = counts.entry(name).or_insert((0, 0));
                entry.0 += 1;
                if error_map.get(&id).copied().unwrap_or(false) {
                    entry.1 += 1;
                }
            }
        }
    }

    let mut out: Vec<ToolUsage> = counts
        .into_iter()
        .map(|(tool_name, (invocation_count, error_count))| ToolUsage {
            tool_name,
            invocation_count,
            error_count,
        })
        .collect();
    out.sort_by(|a, b| {
        b.invocation_count
            .cmp(&a.invocation_count)
            .then_with(|| a.tool_name.cmp(&b.tool_name))
    });
    out
}

// Phase A test specification for the Tool usage aggregator (design doc § Implementation
// :: Step 6). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub struct ToolUsage { tool_name: String, invocation_count: usize,
//      error_count: usize }` matching the entry in § JSON Output Schema's
//     `tools[]` array.
//   - `pub fn extract_tools(records: &[JsonlRecord]) -> Vec<ToolUsage>` —
//     count `tool_use.name` and pair each with the matching
//     `tool_result.is_error` (via `tool_use_id`). Sort by
//     `invocation_count` descending; ties broken by alphabetical `tool_name`.
//     Per § Tools-section composition, `Skill` and `Task` are themselves
//     `tool_use` blocks and DO appear as their own rows in the output.

#[cfg(test)]
mod tests {
    use super::{ToolUsage, extract_tools};
    use crate::parser::record::{JsonlRecord, parse_session};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn load_records(fixture: &str) -> Vec<JsonlRecord> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        parse_session(&path)
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap_or_else(|e| panic!("fixture {fixture} must parse: {e}"))
    }

    // ---------------------------------------------------------------------
    // Counting test — fixture `tools-mix.jsonl` contains:
    //   Bash : 3 invocations, 1 error  (tu2's tool_result has is_error=true)
    //   Read : 2 invocations, 0 errors
    //   Edit : 2 invocations, 1 error  (tu7's tool_result has is_error=true)
    //   Skill: 1 invocation,  0 errors
    //   Task : 1 invocation,  0 errors
    // Skill and Task appear as their own rows per § Tools-section composition.
    // ---------------------------------------------------------------------

    #[test]
    fn counts_invocations_and_errors_per_tool_name() {
        let records = load_records("tools-mix.jsonl");
        let usages = extract_tools(&records);

        let lookup: HashMap<&str, &ToolUsage> =
            usages.iter().map(|u| (u.tool_name.as_str(), u)).collect();

        let bash = lookup.get("Bash").unwrap_or_else(|| {
            panic!(
                "Bash row must be present; got rows {:?}",
                row_names(&usages)
            )
        });
        assert_eq!(bash.invocation_count, 3, "Bash invocation count");
        assert_eq!(bash.error_count, 1, "Bash error count");

        let read = lookup
            .get("Read")
            .unwrap_or_else(|| panic!("Read row must be present"));
        assert_eq!(read.invocation_count, 2);
        assert_eq!(read.error_count, 0);

        let edit = lookup
            .get("Edit")
            .unwrap_or_else(|| panic!("Edit row must be present"));
        assert_eq!(edit.invocation_count, 2);
        assert_eq!(edit.error_count, 1);

        let skill = lookup.get("Skill").unwrap_or_else(|| {
            panic!(
                "Skill row must be present — Skill is itself a `tool_use` block per § Tools-section composition"
            )
        });
        assert_eq!(skill.invocation_count, 1);
        assert_eq!(skill.error_count, 0);

        let task = lookup.get("Task").unwrap_or_else(|| {
            panic!(
                "Task row must be present — Task is itself a `tool_use` block per § Tools-section composition"
            )
        });
        assert_eq!(task.invocation_count, 1);
        assert_eq!(task.error_count, 0);

        assert_eq!(
            usages.len(),
            5,
            "exactly 5 distinct tool names in the fixture; got {:?}",
            row_names(&usages)
        );
    }

    // ---------------------------------------------------------------------
    // Sort test — by `invocation_count` descending; ties broken by
    // alphabetical `tool_name`. The fixture is deliberately constructed
    // with two ties (Edit/Read at count 2, Skill/Task at count 1) so the
    // tie-break rule is unambiguously exercised.
    // ---------------------------------------------------------------------

    #[test]
    fn sorts_descending_by_invocation_count_with_alphabetical_tie_break() {
        let records = load_records("tools-mix.jsonl");
        let usages = extract_tools(&records);
        let order: Vec<&str> = usages.iter().map(|u| u.tool_name.as_str()).collect();
        assert_eq!(
            order,
            vec!["Bash", "Edit", "Read", "Skill", "Task"],
            "expected order is by invocation_count desc, then tool_name asc; \
             Bash(3) > Edit(2)=Read(2) > Skill(1)=Task(1), \
             and within each tie Edit<Read and Skill<Task alphabetically"
        );
    }

    fn row_names(usages: &[ToolUsage]) -> Vec<&str> {
        usages.iter().map(|u| u.tool_name.as_str()).collect()
    }
}
