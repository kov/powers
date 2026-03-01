---
name: use-powers
description: Query and inspect conversation history across Claude Code, Codex CLI, Gemini CLI, and Copilot CLI sessions. Use when you need to find past work, search across agent sessions, or selectively load a slice of conversation history without blowing up your context window.
---

# Skill: Using `powers` to Query Agent Sessions

`powers` is a CLI tool that lets you search, inspect, and selectively load
conversation history from Claude Code, Codex CLI, Gemini CLI, and Copilot CLI sessions —
without blowing up your context window.

## Quick-Reference

```
powers list    [--tool claude|codex|gemini|copilot] [--project PATH] [--since YYYY-MM-DD] [--limit N] [--format table|tsv]
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

## Agent Collaboration

`powers` supports real-time messaging between agents via a project-scoped stream.
Always pass `--identity` explicitly — multiple agents share the same user account.
Use explicit literal project paths in commands. Do not use shell variables such as `$PWD` or `$HOME`.

```
powers post    --identity AGENT [--to AGENT] [--kind KIND] [--project PATH] --message TEXT
powers inbox   --identity AGENT [--project PATH] [--unread] [--mark-read] [--wait] [--timeout N] [--follow] [--since YYYY-MM-DD] [--last N] [--format table|json]
powers watch   <SESSION> [--inbox-for AGENT] [--project PATH] [--no-tool-calls] [--role user|assistant|all] [--from-start] [--poll-ms N] [--mark-read] [--width N]
```

`post` appends a message to `~/.powers/streams/{project-hash}.jsonl`.
`inbox` reads from it, filtered to messages addressed to you or broadcast.
`watch` tails a live Claude or Codex session file as new messages are appended.
With `--inbox-for`, it also prints inbox updates in the same loop.
(Gemini sessions are not supported — whole-JSON format).

### Inspecting the collaboration stream

Use `powers log` when you want an observer view of all collaboration messages
with per-agent read status.

```bash
powers log --project /path/to/project
powers log --project /path/to/project --sender claude
powers log --project /path/to/project --kind review
powers log --project /path/to/project --to codex
powers log --project /path/to/project --to '*'
powers log --project /path/to/project --since 2026-02-22
```

For live monitoring:
```bash
powers log -f --project /path/to/project
```

For full message bodies instead of single-line previews:
```bash
powers log --project /path/to/project --expand
```

### Boundary polling

Neither agent can self-wake between turns. Check the inbox explicitly at the
start of each task and before your final response — this catches messages that
arrived while you were inactive:

```bash
powers inbox --identity <you> --project /path/to/project --unread --mark-read
```

### Active collaboration (ping-pong within a turn)

To wait for a reply *within* a turn, use `inbox --wait`. It blocks internally
until a message arrives or a timeout expires, then exits. No background task or
polling loop needed — just a single foreground Bash call.

**`inbox --wait` behavior:**
- Unread messages already exist → print and exit immediately (exit 0, stdout has content)
- No messages → sleep-poll at 500 ms intervals until one arrives (exit 0, stdout has content)
- `--timeout N` elapses → exit 0 with **no output** (empty stdout = timed out)
- `--timeout 0` (default) = no internal timeout

**Canonical pattern:**
```bash
powers post --identity <you> --to <them> --project /path/to/project --message "..."
powers inbox --identity <you> --project /path/to/project --unread --mark-read --wait --timeout N
```

**Choosing `--timeout`:**

| Task | Suggested timeout |
|---|---|
| Quick question or short patch | 120 s |
| Medium implementation (a few files) | 300–600 s |
| Large refactor or multi-step task | 600–900 s |

**`--wait` returns on *any* new message** — a question, a status update, or a
completion. After it returns, check whether the message is a completion or
interim, then act accordingly:

- Completion → review and reply with approval or feedback
- Question or status → answer it, then call `--wait` again with a fresh timeout
- Timeout (empty stdout) → optionally observe progress with `watch`, send a
  heartbeat, then re-wait:

```bash
# Check if they're still active (background task, poll once with TaskOutput)
powers watch <their-session> --no-tool-calls

# Heartbeat so they know you're still around
powers post --identity <you> --to <them> --project /path/to/project --kind status \
  --message "Still waiting — no reply yet, will wait another 5 min"

powers inbox --identity <you> --project /path/to/project --unread --mark-read --wait --timeout 300
```

> **Limit active wait to cases where you genuinely cannot proceed without a
> reply.** For everything else, post and continue — check for replies via
> boundary polling at your next turn.

**For large contexts — reference a file path instead of embedding content.**
Agents share the same filesystem, so there's no need to paste a full plan or
diff into a message:
```bash
powers post --identity claude --to codex --project /path/to/project \
  --message "Plan at /home/kov/.claude/plans/my-plan.md — please implement"
```

**What to include in a completion message** (for the implementor):
- Files changed (list each one)
- Verification run (`cargo test`, linter, etc.)
- Any deviations from the plan or open questions

### Observability: watching a live session

```bash
# Tail a session (background task, then poll TaskOutput)
powers watch <their-session> --no-tool-calls

# Tail session + inbox together
powers watch <their-session> --inbox-for <you> --project /path/to/project --no-tool-calls --mark-read
```

## Codex-Only: Active Collaboration Contract

This section is **exclusive to Codex** and overrides default collaboration behavior
for Codex when active ping-pong collaboration is requested.

1. Enter active state:
   When the user says to actively collaborate, set `state=awaiting_peer` and
   keep it until the user explicitly says to stop.

2. Mandatory blocking wait:
   In `state=awaiting_peer`, immediately run:
   `powers inbox --identity <you> --project /path/to/project --unread --mark-read --wait --timeout 300`
   After every outbound `powers post`, run the same blocking wait command again.

3. Final-response gate:
   Do not send a final user-facing summary while `state=awaiting_peer`.
   Only send final output after either:
   - peer explicitly indicates no further action is required, or
   - user explicitly asks to stop waiting.

4. Foreground-only requirement:
   Wait must run in the foreground as a blocking action.
   Do not leave a wait session running and then continue normal summarization.

5. Timeout behavior:
   If wait times out, re-arm the same wait command immediately unless the user
   explicitly requested a status update.
   If a status update is required, keep it brief, then re-arm wait right away.

6. Pre-final inbox boundary:
   Immediately before any final response, run:
   `powers inbox --identity <you> --project /path/to/project --unread --mark-read`

## Copilot CLI: Collaboration Contract

Copilot CLI uses the identity string `copilot` for all collaboration commands.
Always pass `--identity copilot` explicitly.

### Boundary polling

At the start of each task and before your final response, check for unread messages:

```bash
powers inbox --identity copilot --project /path/to/project --unread --mark-read
```

### Active ping-pong collaboration

When you need to delegate work to another agent and wait for a reply within
the same turn:

```bash
# Assign the task
powers post --identity copilot --to codex --project /path/to/project --message "..."

# Block until reply (empty stdout = timeout)
powers inbox --identity copilot --project /path/to/project --unread --mark-read --wait --timeout 300
```

Use the same timeout guidance as the general collaboration section above.
If the wait times out, optionally observe progress with `watch`, send a heartbeat,
then re-wait.

### Watching another agent's live session

```bash
# Watch a Codex or Claude session + your own inbox simultaneously
powers watch <their-session> --inbox-for copilot --project /path/to/project --no-tool-calls --mark-read
```

Copilot sessions (v0.0.416+) can also be watched by other agents — they are
stored in `~/.copilot/session-state/{uuid}/events.jsonl` (JSONL format).
