use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn powers() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_powers"));
    let f = fixtures();
    cmd.env("POWERS_CLAUDE_DIR", f.join("claude"))
        .env("POWERS_CODEX_DIR", f.join("codex"))
        .env("POWERS_GEMINI_DIR", f.join("gemini"))
        .env("POWERS_COPILOT_DIR", f.join("copilot"));
    cmd
}

fn powers_with_state_dir(state_dir: &TempDir) -> Command {
    let mut cmd = powers();
    cmd.env("POWERS_DIR", state_dir.path());
    cmd
}

fn stdout(o: &Output) -> &str {
    std::str::from_utf8(&o.stdout).unwrap()
}

fn stderr(o: &Output) -> &str {
    std::str::from_utf8(&o.stderr).unwrap()
}

// ── list ─────────────────────────────────────────────────────────────────────

#[test]
fn list_shows_all_tools() {
    let out = powers().args(["list"]).output().unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("claude"), "missing claude: {s}");
    assert!(s.contains("codex"), "missing codex: {s}");
    assert!(s.contains("gemini"), "missing gemini: {s}");
    assert!(s.contains("copilot"), "missing copilot: {s}");
}

#[test]
fn list_filter_by_tool_claude() {
    let out = powers()
        .args(["list", "--tool", "claude"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("aaaaaaaa"), "claude session missing: {s}");
    assert!(!s.contains("bbbbbbbb"), "codex session leaked: {s}");
    assert!(!s.contains("dddddddd"), "gemini session leaked: {s}");
}

#[test]
fn list_filter_by_project() {
    let out = powers()
        .args(["list", "--project", "/test/project"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("aaaaaaaa"), "claude session missing: {s}");
    assert!(!s.contains("bbbbbbbb"), "wrong project leaked: {s}");
}

#[test]
fn list_tsv_format_has_no_header() {
    let out = powers()
        .args(["list", "--format", "tsv", "--tool", "claude"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // TSV has no header line — first word of first line is the UUID
    let first = s.lines().next().unwrap_or("");
    assert!(
        first.starts_with("aaaaaaaa"),
        "expected UUID first in TSV: {first}"
    );
}

#[test]
fn list_message_count_is_correct() {
    let out = powers()
        .args(["list", "--tool", "claude", "--format", "tsv"])
        .output()
        .unwrap();
    let s = stdout(&out);
    // fixture has 6 user+assistant messages (progress line is skipped)
    let fields: Vec<&str> = s.lines().next().unwrap().split('\t').collect();
    let count: usize = fields.last().unwrap().trim().parse().unwrap();
    assert_eq!(count, 6, "wrong message count: {s}");
}

// ── info ──────────────────────────────────────────────────────────────────────

#[test]
fn info_shows_metadata() {
    let out = powers().args(["info", "aaaaaaaa"]).output().unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "info failed: {s}");
    assert!(s.contains("aaaaaaaa-0000-0000-0000-000000000001"));
    assert!(s.contains("claude"));
    assert!(s.contains("/test/project"));
    assert!(s.contains("main")); // git branch
    assert!(s.contains("6")); // message count
}

#[test]
fn info_unknown_prefix_fails() {
    let out = powers().args(["info", "ffffffff"]).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("No session found"));
}

#[test]
fn info_prefix_too_short_fails() {
    let out = powers().args(["info", "aaaa"]).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("at least 8"));
}

// ── show ──────────────────────────────────────────────────────────────────────

#[test]
fn show_last_2_shows_correct_messages() {
    let out = powers()
        .args(["show", "aaaaaaaa", "--last", "2"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "show failed: {s}");
    // Last 2 of 6 messages: index 4 (tool_result) and 5 (citrus)
    assert!(s.contains("[4]"), "msg 4 missing: {s}");
    assert!(s.contains("[5]"), "msg 5 missing: {s}");
    assert!(!s.contains("[3]"), "msg 3 should not appear: {s}");
}

#[test]
fn show_first_2_shows_correct_messages() {
    let out = powers()
        .args(["show", "aaaaaaaa", "--first", "2"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("[0]"), "msg 0 missing: {s}");
    assert!(s.contains("[1]"), "msg 1 missing: {s}");
    assert!(!s.contains("[2]"), "msg 2 should not appear: {s}");
    assert!(s.contains("apples"));
}

#[test]
fn show_from_to_is_inclusive() {
    let out = powers()
        .args(["show", "aaaaaaaa", "--from", "1", "--to", "3"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("[1]"), "msg 1 missing: {s}");
    assert!(s.contains("[2]"), "msg 2 missing: {s}");
    assert!(s.contains("[3]"), "msg 3 missing: {s}");
    assert!(!s.contains("[0]"), "msg 0 should not appear: {s}");
    assert!(!s.contains("[4]"), "msg 4 should not appear: {s}");
}

#[test]
fn show_role_user_filters_assistant_messages() {
    let out = powers()
        .args(["show", "aaaaaaaa", "--role", "user"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // show uses original (unfiltered) message indices; user messages are at 0, 2, 4
    assert!(s.contains("[0]"), "msg 0 missing: {s}");
    assert!(s.contains("[2]"), "msg 2 missing: {s}");
    assert!(s.contains("[4]"), "msg 4 missing: {s}");
    assert!(!s.contains("[1]"), "assistant msg 1 should not appear: {s}");
    assert!(!s.contains("[3]"), "assistant msg 3 should not appear: {s}");
    assert!(!s.contains("[5]"), "assistant msg 5 should not appear: {s}");
    // User message content
    assert!(s.contains("apples"));
    assert!(s.contains("bananas"));
    // No assistant content
    assert!(!s.contains("citrus"));
}

#[test]
fn show_no_tool_calls_drops_pure_tool_messages() {
    let out = powers()
        .args(["show", "aaaaaaaa", "--no-tool-calls"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());

    // msg 3 (assistant) had tool_use + text; with --no-tool-calls, text survives
    assert!(
        s.contains("Here are the bananas."),
        "assistant text stripped: {s}"
    );
    // tool_use label should not appear
    assert!(!s.contains("[tool_call:"), "tool_call leaked: {s}");

    // msg 4 (user) was a pure tool_result — should be dropped entirely
    assert!(!s.contains("bananas output"), "tool_result leaked: {s}");
}

#[test]
fn show_header_reports_correct_slice() {
    let out = powers()
        .args(["show", "aaaaaaaa", "--last", "3"])
        .output()
        .unwrap();
    let s = stdout(&out);
    // Header format: "msgs 3-5 of 6"
    assert!(s.contains("of 6"), "total count wrong: {s}");
    assert!(
        s.contains("3-5") || s.contains("msgs 3"),
        "slice info wrong: {s}"
    );
}

#[test]
fn show_expand_persisted_reads_tool_result_file() {
    let out = powers()
        .args([
            "show",
            "aaaaaaaa",
            "--last",
            "2",
            "--expand-persisted",
            "--max-bytes",
            "24",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "show failed: {s}");
    assert!(s.contains("persisted_path:"), "persisted path missing: {s}");
    assert!(s.contains("banana line 1"), "expanded content missing: {s}");
    assert!(s.contains("truncated"), "expected truncation marker: {s}");
}

// ── search ────────────────────────────────────────────────────────────────────

#[test]
fn search_finds_match_in_correct_session() {
    let out = powers().args(["search", "apples"]).output().unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("aaaaaaaa"), "session header missing: {s}");
    assert!(s.contains("apples"), "match text missing: {s}");
}

#[test]
fn search_no_match_produces_no_output() {
    let out = powers()
        .args(["search", "zzz_no_such_word_zzz"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(stdout(&out).trim().is_empty(), "expected no output");
}

#[test]
fn search_session_only_prints_uuid_per_line() {
    let out = powers()
        .args(["search", "apples", "--session-only", "--tool", "claude"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 1, "expected exactly one line: {s}");
    assert!(lines[0].starts_with("aaaaaaaa"), "expected UUID: {s}");
}

#[test]
fn search_context_includes_surrounding_messages() {
    // "citrus" appears only in msg 5 (last). With --context 1, msg 4 should appear before it.
    let out = powers()
        .args(["search", "citrus", "--context", "1"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // msg 5 is the match (index 4 in streaming, since msg 4/tool_result is index 4);
    // with context=1 the message before citrus (the tool_result at streaming index 4) appears
    assert!(s.contains("citrus"), "match missing: {s}");
    // the pre-context entry immediately before the citrus match should be present
    assert!(
        s.contains("msg 4") || s.contains("msg 5"),
        "pre-context missing: {s}"
    );
}

#[test]
fn search_context_zero_shows_match_only() {
    let out = powers()
        .args(["search", "citrus", "--context", "0"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("citrus"), "match missing: {s}");
    // No surrounding messages
    assert!(!s.contains("bananas"), "context leaked: {s}");
}

#[test]
fn search_overlapping_context_merges_groups() {
    // "bananas" matches msg 2 (user) and msg 3 (assistant text).
    // With --context 1: pre(msg1), match(msg2), msg3 is both post-context AND a match
    // → extends post-context → prints msg4 as well.
    // All of 1..4 should appear in a single contiguous block, not two separate ones.
    let out = powers()
        .args(["search", "bananas", "--context", "1"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // Should see: "I see apples" (msg1 pre-ctx), "bananas" (msg2), "Here are the bananas" (msg3)
    assert!(s.contains("I see apples"), "pre-context missing: {s}");
    assert!(
        s.contains("Here are the bananas"),
        "second match missing: {s}"
    );
    // Only one session header (merged into one block)
    let header_count = s.matches("=== claude").count();
    assert_eq!(header_count, 1, "expected single merged block: {s}");
}

#[test]
fn search_filter_by_tool_excludes_other_tools() {
    let out = powers()
        .args(["search", "grapes", "--tool", "claude"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // "grapes" is in the gemini fixture; searching only claude should find nothing
    assert!(
        s.trim().is_empty(),
        "gemini result leaked into claude search: {s}"
    );
}

#[test]
fn search_case_insensitive_by_default() {
    let out = powers().args(["search", "APPLES"]).output().unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("apples"), "case-insensitive match failed: {s}");
}

#[test]
fn search_case_sensitive_flag() {
    let out = powers()
        .args(["search", "APPLES", "--case-sensitive"])
        .output()
        .unwrap();
    assert!(out.status.success());
    // "APPLES" (uppercase) is not in the fixture — should find nothing
    assert!(
        stdout(&out).trim().is_empty(),
        "unexpected match with --case-sensitive"
    );
}

#[test]
fn search_tool_filter_codex() {
    let out = powers()
        .args(["search", "oranges", "--tool", "codex"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("bbbbbbbb"), "codex session missing: {s}");
    assert!(s.contains("oranges"), "match missing: {s}");
}

#[test]
fn search_tool_filter_gemini() {
    let out = powers()
        .args(["search", "grapes", "--tool", "gemini"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("dddddddd"), "gemini session missing: {s}");
    assert!(s.contains("grapes"), "match missing: {s}");
}

// ── collaboration stream ─────────────────────────────────────────────────────

#[test]
fn post_and_inbox_show_targeted_and_broadcast_messages() {
    let state_dir = tempfile::tempdir().unwrap();

    let out = powers_with_state_dir(&state_dir)
        .args([
            "post",
            "--identity",
            "claude",
            "--to",
            "codex",
            "--project",
            "/test/project",
            "--message",
            "please review parser changes",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "post failed: {}", stdout(&out));

    let out = powers_with_state_dir(&state_dir)
        .args([
            "post",
            "--identity",
            "claude",
            "--project",
            "/test/project",
            "--message",
            "build green",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "post failed: {}", stdout(&out));

    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--unread",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "inbox failed: {s}");
    assert!(
        s.contains("please review parser changes"),
        "targeted missing: {s}"
    );
    assert!(s.contains("build green"), "broadcast missing: {s}");
}

#[test]
fn inbox_mark_read_advances_cursor() {
    let state_dir = tempfile::tempdir().unwrap();

    let out = powers_with_state_dir(&state_dir)
        .args([
            "post",
            "--identity",
            "claude",
            "--to",
            "codex",
            "--project",
            "/test/project",
            "--message",
            "one",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--unread",
            "--mark-read",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(stdout(&out).contains("one"));

    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--unread",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        stdout(&out).trim().is_empty(),
        "expected no unread messages: {}",
        stdout(&out)
    );
}

#[test]
fn inbox_last_limits_visible_output() {
    let state_dir = tempfile::tempdir().unwrap();

    for body in ["one", "two", "three"] {
        let out = powers_with_state_dir(&state_dir)
            .args([
                "post",
                "--identity",
                "claude",
                "--to",
                "codex",
                "--project",
                "/test/project",
                "--message",
                body,
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "post failed: {}", stdout(&out));
    }

    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--last",
            "1",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "inbox failed: {s}");
    assert!(s.contains("three"), "expected most recent message: {s}");
    assert!(!s.contains("one"), "old message should be omitted: {s}");
    assert!(!s.contains("two"), "old message should be omitted: {s}");
}

#[test]
fn inbox_wait_exits_immediately_if_message_present() {
    let state_dir = tempfile::tempdir().unwrap();

    // Post a message first so the stream is non-empty
    let out = powers_with_state_dir(&state_dir)
        .args([
            "post",
            "--identity",
            "claude",
            "--to",
            "codex",
            "--project",
            "/test/project",
            "--message",
            "ready when you are",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "post failed: {}", stdout(&out));

    // inbox --wait should find the message immediately and exit with output
    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--unread",
            "--wait",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "inbox --wait failed: {s}");
    assert!(
        s.contains("ready when you are"),
        "expected message in output: {s}"
    );
}

#[test]
fn inbox_wait_timeout_exits_with_no_output() {
    let state_dir = tempfile::tempdir().unwrap();

    // No messages posted — --wait --timeout 1 should exit in ~1s with no output
    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--unread",
            "--wait",
            "--timeout",
            "1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "inbox --wait --timeout failed");
    assert!(
        stdout(&out).trim().is_empty(),
        "expected no output on timeout: {}",
        stdout(&out)
    );
}

#[test]
fn inbox_format_json_outputs_jsonl_records() {
    let state_dir = tempfile::tempdir().unwrap();

    let out = powers_with_state_dir(&state_dir)
        .args([
            "post",
            "--identity",
            "claude",
            "--to",
            "codex",
            "--kind",
            "note",
            "--project",
            "/test/project",
            "--message",
            "json payload",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "post failed: {}", stdout(&out));

    let out = powers_with_state_dir(&state_dir)
        .args([
            "inbox",
            "--identity",
            "codex",
            "--project",
            "/test/project",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "inbox failed: {s}");

    let line = s.lines().next().unwrap_or("");
    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(value["from"], "claude");
    assert_eq!(value["to"], "codex");
    assert_eq!(value["kind"], "note");
    assert_eq!(value["body"], "json payload");
}

// ── copilot ───────────────────────────────────────────────────────────────────

#[test]
fn list_filter_by_tool_copilot() {
    let out = powers()
        .args(["list", "--tool", "copilot"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("cccccccc"), "copilot session missing: {s}");
    assert!(!s.contains("aaaaaaaa"), "claude session leaked: {s}");
    assert!(!s.contains("bbbbbbbb"), "codex session leaked: {s}");
}

#[test]
fn list_copilot_message_count() {
    let out = powers()
        .args(["list", "--tool", "copilot", "--format", "tsv"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // First row (most recent) is the session with events.jsonl (6 messages)
    let first_row = s.lines().next().unwrap_or("");
    assert!(
        first_row.contains("cccccccc"),
        "expected copilot session first: {first_row}"
    );
    let fields: Vec<&str> = first_row.split('\t').collect();
    let count: usize = fields.last().unwrap().trim().parse().unwrap();
    assert_eq!(
        count, 7,
        "expected 7 messages (3 user + 3 assistant + 1 tool result): {s}"
    );
}

#[test]
fn list_copilot_legacy_session_has_zero_messages() {
    let out = powers()
        .args(["list", "--tool", "copilot", "--format", "tsv"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    // Legacy session (no events.jsonl) should appear with 0 messages
    let legacy_row = s
        .lines()
        .find(|l| l.contains("cccccccc-0000-0000-0000-000000000002"))
        .unwrap_or("");
    assert!(!legacy_row.is_empty(), "legacy session missing: {s}");
    let fields: Vec<&str> = legacy_row.split('\t').collect();
    let count: usize = fields.last().unwrap().trim().parse().unwrap();
    assert_eq!(count, 0, "legacy session should have 0 messages: {s}");
}

#[test]
fn info_copilot_shows_metadata() {
    let out = powers()
        .args(["info", "cccccccc-0000-0000-0000-000000000001"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "info failed: {s}");
    assert!(s.contains("cccccccc-0000-0000-0000-000000000001"));
    assert!(s.contains("copilot"));
    assert!(s.contains("/test/project"));
    assert!(s.contains("main")); // git branch from events.jsonl
    assert!(s.contains("6")); // message count
}

#[test]
fn show_copilot_last_2() {
    let out = powers()
        .args([
            "show",
            "cccccccc-0000-0000-0000-000000000001",
            "--last",
            "2",
            "--no-tool-calls",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "show failed: {s}");
    assert!(s.contains("cherries"), "third user message missing: {s}");
    assert!(
        s.contains("Cherries are red"),
        "third assistant message missing: {s}"
    );
    // Earlier messages should not appear
    assert!(!s.contains("apples"), "apples leaked: {s}");
}

#[test]
fn search_tool_filter_copilot() {
    let out = powers()
        .args(["search", "bananas", "--tool", "copilot"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("cccccccc"), "copilot session missing: {s}");
    assert!(s.contains("bananas"), "match missing: {s}");
}

#[test]
fn search_copilot_excludes_other_tools() {
    // "apples" appears in claude fixture too — filter to copilot only
    let out = powers()
        .args(["search", "apples", "--tool", "copilot"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success());
    assert!(s.contains("cccccccc"), "copilot session missing: {s}");
    assert!(!s.contains("aaaaaaaa"), "claude session leaked: {s}");
}

#[test]
fn search_copilot_no_tool_calls_in_show() {
    // Assistant message in fixture has a tool call; show --no-tool-calls should strip it
    let out = powers()
        .args([
            "show",
            "cccccccc-0000-0000-0000-000000000001",
            "--no-tool-calls",
        ])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "show failed: {s}");
    // "bash" is a tool call name — should not appear in output
    assert!(!s.contains("\"bash\""), "tool call name leaked: {s}");
    // Prose content should still be present
    assert!(
        s.contains("Here are the bananas"),
        "assistant prose missing: {s}"
    );
}

// ── tool-result ──────────────────────────────────────────────────────────────

#[test]
fn tool_result_finds_and_resolves_persisted_output() {
    let out = powers()
        .args(["tool-result", "tool-1", "--max-bytes", "24"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(out.status.success(), "tool-result failed: {s}");
    assert!(
        s.contains("tool_result tool-1"),
        "tool result header missing: {s}"
    );
    assert!(s.contains("persisted_path:"), "persisted path missing: {s}");
    assert!(s.contains("banana line 1"), "resolved content missing: {s}");
}
