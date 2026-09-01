# Agent Instructions

This file is the repository-local agent guide. Use the uppercase filename `AGENTS.md` so coding agents can find it by convention.

`MEMORY.md` is the durable context store for this repository. Chat context may be compressed or lost, so anything that future agents need to retain must be written there.

## Startup Checklist

- Read this `AGENTS.md` file at the start of work in this repository.
- Read `MEMORY.md` before planning or making changes.
- Start with these `MEMORY.md` sections: `Active Handoff`, `Current State`, `Open Follow-ups`, and `Do Not Forget`.
- If context was compressed, the session was resumed, or prior work is unclear, follow the `Compression Recovery` workflow below.
- When in doubt, check `MEMORY.md` before assuming project history, user preferences, architecture, setup steps, or unresolved work.

## Memory-First Rule

Use `MEMORY.md` as the source of retained project context. If chat history conflicts with `MEMORY.md`, verify the current repo state before acting and update `MEMORY.md` with any correction.

## Source Of Truth

When sources conflict, prefer them in this order:

1. Current files in the workspace.
2. Latest command output or freshly verified tool output.
3. `MEMORY.md`.
4. Chat history.

Treat browser state, email state, authentication state, and external service results as stale unless verified in the current session. Treat documented repo structure, user preferences, and decisions as durable unless current files or fresh verification contradict them.

## When To Update `MEMORY.md`

Update `MEMORY.md` consistently throughout the work, not only at the end. Add or revise entries whenever information should survive context loss, including:

- User preferences or instructions that affect future work.
- Project setup, commands, environment details, credentials-handling notes, or workflow requirements.
- Architecture decisions, implementation plans, and important tradeoffs.
- Bugs found, fixes applied, test results, and known regressions.
- Unresolved questions, blocked tasks, follow-ups, and assumptions.
- File paths, modules, APIs, or integration details that future agents should not have to rediscover.

## Checkpoint Rules

Update `MEMORY.md` after any meaningful checkpoint:

- A durable decision is made.
- A non-obvious setup or command is discovered.
- A bug is diagnosed or fixed.
- Tests or verification produce important results.
- Work is paused, blocked, or handed off.
- A previous memory is found to be stale or wrong.

## Entry Quality

- Keep entries concise, factual, and easy to scan.
- Prefer dated entries with short headings.
- Include file paths, commands, and test names when they matter.
- Use `Last verified YYYY-MM-DD` for facts that can drift.
- Do not store secrets, tokens, passwords, private keys, or sensitive personal data.
- For email-related work, do not store full email bodies, private addresses, case details, or sensitive agency data. Store only brief summaries, message IDs, dates, and action state when needed.
- Do not turn `MEMORY.md` into a transcript. Store conclusions, decisions, setup details, blockers, and verified results.
- If an old memory becomes inaccurate, correct it in place or add a clearly dated correction.
- Keep `Active Handoff` and `Current State` short. Move old completed details into `Completed Work` or `Archive`.

## Memory Entry Template

Use this format when adding a meaningful dated entry:

```md
### YYYY-MM-DD - Short Title

- Context:
- Decision:
- Files:
- Commands/tests:
- Result:
- Follow-up:
```

## Compression Recovery

When context is missing, stale, compressed, or ambiguous:

1. Read this `AGENTS.md`.
2. Read `MEMORY.md`, especially `Active Handoff`, `Current State`, `Open Follow-ups`, and `Do Not Forget`.
3. Inspect the files related to the current task.
4. Re-run or inspect relevant verification only when needed.
5. Update `MEMORY.md` with the recovered state before making broad changes.

## Before Final Response

Before finishing substantial work:

- Review whether `MEMORY.md` needs a final update.
- Record important files changed, decisions made, test results, and follow-ups.
- Make sure `Active Handoff`, `Current State`, `Verification Ledger`, and `Open Follow-ups` reflect the actual handoff state.

## Default Operating Rule

If there is any uncertainty about retained context, read `MEMORY.md` first.
