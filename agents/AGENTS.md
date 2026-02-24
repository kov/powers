# Important rules

## Date and time
- when thinking about dates or time (e.g. to decide if something is current / recent), do not assume you know when we are,
  run `date` or use a tool to figure out

## Running shell commands
- always favor using tools rather than shell commands
- do not use shell variables in commands you run or suggest (for example `$PWD` or `$HOME`); use explicit literal paths

## Rust crates
- when adding new dependencies always verify you are using the latest stable version
- verify API documentation in https://docs.rs/ or using the cratedocs tools
- do not use cargo doc, you cannot see the output

## Making changes
- when in doubt: ask
- deviating from practice, exceptions, changes to design are all acceptable with a good reason, but require confirmation

## Important: `cat` is an alias to `bat`
- disable paging with --paging=never
- disable special formatting with -p
- if you really want `cat`, use the absolute path `/bin/cat`

## git
- never use git add -A or similar, be intentional and specific when adding files to git
- I very often leave important and private files in git repositories that are not on .gitignore, they should not be added
- always run whatever formatting and linting tools are specified for the project before committing, if unsure which one
  or how to run it, ask

# Maintaining project-wide guidance files
- Keep guidance short and high-signal.
- Include only rules that are critical across most tasks (durable constraints, invariants, safety checks).
- Remove content that is task-specific, historical, redundant, or easy to discover via --help, quick file exploration, or nearby code.
- Prefer pointing to canonical docs/files over duplicating long implementation details.

# Skills Index

The following skills are available. Keep them in mind while doing your work,
and load when it looks like it could be helpful for what you are doing.

## use-powers

Query and inspect conversation history across Claude Code, Codex CLI, and
Gemini CLI sessions. Use this skill to find past work, load specific message
ranges into context, and search across all agents without blowing up your
context window.
