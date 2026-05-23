# cc-turntable

A local-only, post-hoc CLI for auditing Claude Code sessions. Reads the JSONL log Claude Code already writes to disk and surfaces what actually happened: skills fired, subagents spawned, tools used, errors, interventions.

## Install

```
git clone https://github.com/lil-lon/cc-turntable
cd cc-turntable/ccturn
cargo install --path .
```

The binary lands at `~/.cargo/bin/ccturn`. Default log root is `$CLAUDE_CONFIG_DIR/projects/` (falls back to `~/.claude/projects/` when unset). Pass `--log-root PATH` to override.

Exit codes: `0` success, `1` not-found (session / project / log root), `2` parser failure, `64` usage error.

## `ccturn crates` — list projects

List every project directory under the log root with session counts, latest timestamp, and ground-truth cwd. Sorted most-recent first.

```
ccturn crates                    # human view
ccturn crates --json             # single compact JSON object
```

Example:

```
$ ccturn crates
Log root  /Users/you/.claude/projects   (3 projects)

  -Users-you-cc-turntable        42 sessions   latest 2026-05-22T18:30:00Z   /Users/you/cc-turntable
  -Users-you-other-repo           3 sessions   latest 2026-05-21T10:15:00Z   /Users/you/other-repo
  -tmp-scratch                    0 sessions   latest none                  /tmp/scratch
```

## `ccturn tracks PROJECT` — list sessions in a project

Lists sessions in one project as a git-log-style block per session (UUID / status / date / title / subagent tree). `PROJECT` is the encoded-cwd token from `ccturn crates` output — copy-paste it verbatim.

```
ccturn tracks <PROJECT>              # default (git-log-style multi-line block)
ccturn tracks <PROJECT> --oneline    # one row per session (git-log --oneline analogue)
ccturn tracks <PROJECT> -n 5         # cap at the 5 most recent
ccturn tracks <PROJECT> --json       # single compact JSON object (incompatible with --oneline)
```

Example:

```
$ ccturn tracks -Users-you-cc-turntable -n 1
Project   /Users/you/cc-turntable
Encoded   -Users-you-cc-turntable
Sessions  42 total   (showing 1)

session cbb44fe2-744e-4aee-a42d-fe87703da4b3
Status: success
Date:   2026-05-22T18:30:00Z

    gh-cli skill orientation

    Subagents (2):
      ├─ Explore agent-abc-123   completed   11.6s   20792 tok   bash=4
      │     Verify Skill and Task tool fields in JSONL
      └─ Plan agent-def-456      completed    7.2s   14530 tok   read=12
            Phase 1 implementation plan
```

Status is one of `success` / `error` / `aborted` / `unknown`, inferred from the session's tool_use / tool_result ladder.

## `ccturn spin SESSION_ID` — single-session report

Plays through one session: skills fired, subagents spawned, tools used (top 10), categorized errors (UserRejection / PermissionDenied / HookBlock / Technical), and user interventions.

```
ccturn spin <SESSION_ID>                   # human-readable report
ccturn spin <SESSION_ID> --json            # single compact JSON object
ccturn spin <SESSION_ID> --project <enc>   # scope to one project subdirectory
```

Example:

```
$ ccturn spin cbb44fe2-744e-4aee-a42d-fe87703da4b3
Session  cbb44fe2-744e-4aee-a42d-fe87703da4b3
Project  /Users/you/cc-turntable
Span     2026-05-22T18:30:00Z → 2026-05-22T19:12:04Z  (42m 4s)
Records  1234 lines

== Skills (1) ==
  gh-cli  1 invocation, 0 inner errors, window 1 tool uses

== Tools (top 10 by use) ==
  Bash  111 invocations, 19 errors
  Read   77 invocations, 0 errors
  Edit   38 invocations, 3 errors
  ...

== Errors (23) ==
  UserRejection (2): ...
  PermissionDenied (3): ...
  HookBlock (5): ...
  Technical (13): ...
```

## License

MIT. See [LICENSE](LICENSE).
