---
name: use-powers
description: Query and inspect conversation history across Claude Code, Codex CLI, and Gemini CLI sessions. Use when you need to find past work, search across agent sessions, or selectively load a slice of conversation history without blowing up your context window.
---

# Skill: Using `powers` to Query Agent Sessions

`powers` is a CLI tool that lets you search, inspect, and selectively load
conversation history from Claude Code, Codex CLI, and Gemini CLI sessions —
without blowing up your context window.

## Quick-Reference

```
powers list    [--tool claude|codex|gemini] [--project PATH] [--since YYYY-MM-DD] [--limit N] [--format table|tsv]
powers search  <PATTERN> [--tool ...] [--project ...] [--since ...] [--context N] [--max-matches N] [--case-sensitive] [--session-only]
powers info    <SESSION>
powers show    <SESSION> [--last N] [--first N] [--from N] [--to N] [--role user|assistant|all] [--no-tool-calls] [--width N]
```

`SESSION` is a UUID prefix of at least 8 characters. Powers warns if ambiguous.

## When to Use It

- You need to recall how a problem was solved in a previous session
- You want to load a specific slice of a long past session into your context
- You want to find which session discussed a particular topic across all agents
- You're continuing work that was partly done by a different agent (Claude, Codex, Gemini)

## Recommended Workflow: Find → Info → Tail → Load Range

### 1. Find relevant sessions

```bash
# Narrow by agent and project first (fast — reads only metadata)
powers list --tool claude --project /home/kov/Projects/myproject

# Search across all agents for a keyword
powers search "authentication bug" --session-only

# Search in a specific project since last week
powers search "rate limit" --project /home/kov/Projects/api --since 2026-02-14
```

### 2. Get session metadata

```bash
powers info 5241ab6c
```

This shows: tool, project, git branch, start/end time, message count, file path.

### 3. Preview the tail to orient yourself

```bash
# See the last 10 messages (no tool noise)
powers show 5241ab6c --last 10 --no-tool-calls
```

### 4. Load a specific range

```bash
# Load messages 15 through 30 (stable 0-based indices)
powers show 5241ab6c --from 15 --to 30

# Load a specific role only (e.g., just the user prompts as a summary)
powers show 5241ab6c --role user --first 20
```

## Efficiency Tips

- **Use `--session-only` first** when searching: it outputs only session IDs and is
  much faster than printing full context. Then use `show` on the sessions you care about.

- **Use `--no-tool-calls`** to read prose without tool call noise — critical for large
  sessions with hundreds of tool calls.

- **Use `--last 20` before loading a range** — it's cheap and tells you whether the
  session ended where you think it did.

- **Message indices are stable**: `--from` and `--to` refer to the 0-based position
  in the full message list. Use `info` to see the total count, then compute ranges.

- **`--context N`** (default 2) in `search` shows N messages before and after each
  match. Set to 0 for match-only output; increase for more surrounding context.

## Output Format Notes

### `list`

```
SESSION-ID       TOOL    PROJECT                   DATE        MESSAGES
5241ab6c-…       claude  /home/kov/Projects/powers 2026-02-21  47
```

Use `--format tsv` for scripting.

### `search`

```
=== claude 5241ab6c  /home/kov/Projects/powers  2026-02-21 ===
[msg 12 / user / 2026-02-21T13:45:01Z]
  Design a detailed implementation plan for a Rust CLI tool...
```

### `show`

```
=== claude 5241ab6c-… | /home/kov/Projects/powers | 2026-02-21 | msgs 12-14 of 47 ===

[12] user 2026-02-21T13:45:01Z
  Design a detailed implementation plan...

[13] assistant 2026-02-21T13:46:00Z
  Here's the plan...
```

The `[N]` index is the message's original 0-based position in the full session,
stable across different `--role` filters. Use `--role all` (default) when you
need to use the index with `--from`/`--to`.

## Example: Continuing Another Agent's Work

```bash
# Find what Codex was doing on the pinchy project
powers list --tool codex --project /home/kov/Projects/pinchy

# See the last session's summary (last 15 messages, no tools)
powers show 019c7584 --last 15 --no-tool-calls

# Find where a specific function was discussed
powers search "format_return_value" --tool codex --project /home/kov/Projects/pinchy

# Load the surrounding context once you find the relevant message index
powers show 019c7584 --from 42 --to 55
```
