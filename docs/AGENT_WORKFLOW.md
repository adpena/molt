# Multi-Agent Workflow

This doc standardizes parallel AI agent work on Molt.

## 1) Access and tooling
- Agents may use `gh` (GitHub CLI) and git over SSH.
- Agents are expected to create branches, open PRs, and merge when approved.
- Proactively commit work in logical chunks with clear messages.

## 2) Locking and ownership
- Use `AGENT_LOCKS.md` to claim files/directories before editing.
- Claims must be explicit and removed promptly when work finishes.

## 3) Work partitioning
- Assign each agent a scoped area (runtime/frontend/docs/tests) and avoid overlap.
- If cross‑cutting changes are required, coordinate and update locks first.

## 4) Communication rules
- Always announce: scope, files touched, and expected tests.
- Keep status updates short and explicit.
- Flag any risky changes early.

## 5) Quality gates
- Run extensive linting and tests before PRs or merges.
- Prefer `tools/dev.py lint` + `tools/dev.py test`, plus relevant `cargo check/test`.
- Do not merge if tests are failing unless explicitly approved.

## 6) Merge discipline
- Merge only after tests pass and conflicts are resolved.
- If two agents touch the same area, rebase and re‑validate.
