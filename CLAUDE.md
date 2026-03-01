# Agent Guidance: `powers` Repository

## Project Overview

`powers` is a Rust CLI tool for querying AI agent session history (Claude Code,
Codex CLI, Gemini CLI). It also serves as the home for global agent skills and
guidance files used across all of the repository owner's agents.

## Read-Only Invariant

**Never write to, modify, or delete files in:**
- `~/.claude/` (Claude Code session storage)
- `~/.codex/` (Codex CLI session storage)
- `~/.gemini/` (Gemini CLI session storage)

`powers` is a read-only query tool. Session files are owned by their respective
agents. Any write would corrupt a live session or lose conversation history.

## Streaming Discipline

When working on search or show functionality, never load full sessions
into memory when a streaming approach is possible:

- JSONL files: scan line by line, parse only lines that match a pre-filter,
  use a ring buffer of `2*context+1` messages maximum
- JSON files (Gemini): load only the `messages` array, process with ring buffer

The `Session` struct is for full loads (show command only). Search must stream.

## Code Conventions

- `anyhow::Result` everywhere — no custom error types
- All terminal output goes through `output.rs` — no ad-hoc `println!` in commands
- Parsers expose both `discover()` (fast, metadata-only) and `load()` (full parse)
- For streaming search, parsers additionally expose `pub` single-record parse
  functions (`parse_message_from_record`, `parse_nested_record_pub`, etc.)

## Module Structure

```
src/
  main.rs           — CLI dispatch only
  cli.rs            — clap Derive structs
  config.rs         — Path resolution (~/.claude etc.) with env var overrides:
                       POWERS_CLAUDE_DIR, POWERS_CODEX_DIR, POWERS_GEMINI_DIR, POWERS_COPILOT_DIR, POWERS_DIR
  session.rs        — Core types: SessionMeta, Session, Message, Role, Tool
  parsers/
    mod.rs          — Parser trait: discover() + load()
    claude.rs       — Claude JSONL parser
    codex.rs        — Codex JSONL parser (old flat + new nested formats)
    gemini.rs       — Gemini JSON parser
    copilot.rs      — Copilot CLI JSONL parser (events.jsonl format)
  commands/
    list.rs         — powers list
    search.rs       — powers search (streaming, ring-buffer context)
    show.rs         — powers show (slice logic)
    info.rs         — powers info + shared discover_all() + resolve_session_meta()
  output.rs         — All terminal formatting

CLAUDE.md             — This file (repo root)
agents/
  AGENTS.md         — Skill index (by name, not path)

skills/
  use-powers/SKILL.md  — Skill: how agents use powers
```

## Adding a New Parser (4-Step Checklist)

1. **`src/parsers/<tool>.rs`** — implement `discover()` and `load()`. Expose
   any single-record parse functions needed by streaming search as `pub`.

2. **`src/parsers/mod.rs`** — add `pub mod <tool>;`

3. **`src/commands/search.rs`** — add the tool variant to `search_session()`,
   implement a streaming scan function analogous to `search_jsonl()` or
   `search_gemini()`.

4. **`src/commands/{list,info,show}.rs`** — wire up the new parser in
   `discover_all()` / `discover_filtered()` / `load_session()`.

Nothing else needs to change.

## Agent Collaboration (powers post / inbox)

The `powers post` and `powers inbox` commands use a project-scoped collaboration
stream at `~/.powers/streams/{project-hash}.jsonl`. Always pass `--identity` explicitly
(multiple agents share the same user account).

**Your identity for `--identity` depends on which harness you are running in:**
- pi → `--identity pi`
- Claude Code → `--identity claude`
- Copilot CLI → `--identity copilot`
- Codex CLI   → `--identity codex`
- Gemini CLI  → `--identity gemini`

**Boundary polling — do this at the start of each task and before your final response:**
```bash
# Claude Code:
powers inbox --identity claude --unread --mark-read --project /path/to/project
# Copilot CLI:
powers inbox --identity copilot --unread --mark-read --project /path/to/project
```

**Ping-pong — for active collaboration where you need a reply before continuing:**
```bash
# Post a task or question (substitute your identity for --identity)
powers post --identity claude --to codex --project /path/to/project --message "..."
# Block until reply; empty stdout = timeout
powers inbox --identity claude --project /path/to/project --unread --mark-read --wait --timeout 300
```

Use explicit literal project paths in commands. Do not use shell variables such as `$PWD` or `$HOME`.

See `skills/use-powers/SKILL.md` for full guidance including timeout calibration,
handling interim messages, and what to include in completion notices.

Inspect the collaboration stream with read-status annotations:
```bash
powers log --project PATH       # inspect stream with per-agent read status
powers log -f --project PATH    # follow in-place, refreshes as cursors advance
```

## Testing

- Unit tests live in the same file as the code they test (`#[cfg(test)]`)
- Use `tempfile::NamedTempFile` for fixture sessions in parser tests
- Integration fixtures go in `tests/fixtures/` (minimal real-format sessions)
- Run with `cargo test`

## Session File Format Reference

| Tool    | Path pattern                                       | Format |
|---------|---------------------------------------------------|--------|
| Claude  | `~/.claude/projects/*/*.jsonl`                    | JSONL  |
| Codex   | `~/.codex/sessions/rollout-*.jsonl`               | JSONL (old flat) |
| Codex   | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`    | JSONL (new nested) |
| Gemini  | `~/.gemini/tmp/*/chats/*.json`                    | JSON object |
| Copilot | `~/.copilot/session-state/*/events.jsonl`         | JSONL (v0.0.416+) |

**Claude line 0**: `{type:"system", sessionId, cwd, gitBranch, timestamp}`
**Codex old line 0**: `{id, timestamp, instructions, git:{branch}}`
**Codex new line 0**: `{type:"session_meta", payload:{id, cwd, timestamp}}`
**Gemini top-level**: `{sessionId, projectHash, startTime, lastUpdated, messages[]}`
**Copilot session.start**: `{type:"session.start", data:{sessionId, startTime, context:{cwd, gitRoot, branch}}, timestamp}`

**Claude message types to keep**: `"user"`, `"assistant"` (skip all others)
**Codex old**: `{type:"message", role:"user"|"assistant", content:[{type,text}]}`
**Codex new**: `{type:"response_item", payload:{type:"message", role, content:[...]}}`
**Gemini**: `{type:"user"|"gemini", content: string|[{text}], toolCalls?:[...]}`
**Copilot user**: `{type:"user.message", data:{content}, timestamp}`
**Copilot assistant**: `{type:"assistant.message", data:{content, toolRequests:[{name,arguments}]}, timestamp}`

Copilot metadata is in `workspace.yaml` alongside `events.jsonl`; sessions predating v0.0.416 have only `workspace.yaml` (no message content).
