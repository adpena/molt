# Minimal Agent Progress Report Format (Molt)
**Status:** Canonical template
**Goal:** A small, consistent report agents can emit repeatedly during long tasks.
**Design:** Human skimmable + machine parsable + resumable.

---

## Requirements
Every report MUST include:
- task identifier and timestamp
- what changed (high-level)
- where outputs/artifacts are
- current state (running/stopped)
- next steps
- exact command(s) to resume

Keep it short. Prefer bullets. Avoid prose.

---

## Template (copy/paste)

```markdown
# Agent Progress Report

## Meta
- Task: <short name>
- Report ID: <YYYYMMDD-HHMMSS>  (UTC recommended)
- Agent: <model/tool name>
- Repo: <path or URL>
- Branch/Commit: <branch> / <short sha>
- Session: <tmux session/window OR process name>
- Status: running | paused | blocked | done

## Summary (<= 3 bullets)
- <what changed / what was achieved>
- <what changed / what was achieved>
- <what changed / what was achieved>

## Outputs (paths are REQUIRED)
- Artifacts:
  - <path>  (e.g., build outputs, generated docs)
- Logs:
  - <path>  (e.g., logs/agents/<task>.log)
- Bench results:
  - <path>  (e.g., logs/benchmarks/<run>.json)

## Changes Made
- Code:
  - <file>: <what changed>
- Docs:
  - <file>: <what changed>
- Config/CI:
  - <file>: <what changed>

## Commands Run (exact)
- <command>
- <command>

## Verification
- Tests:
  - <command> → PASS/FAIL (+ note)
- Lints/format:
  - <command> → PASS/FAIL
- Benchmarks:
  - <command> → PASS/FAIL (+ key numbers if available)

## Key Metrics (if applicable)
- Latency: P50=<..> P95=<..> P99=<..>
- Throughput: <..> ops/s per core
- Memory (RSS): <..> MB
- Size: <..> MB (binary/wasm)
- Notes: <one-line interpretation>

## Blockers / Risks
- <blocker> (owner: <me/you>, severity: low/med/high)
- <risk> (what could go wrong)

## Next Steps (ordered)
1) <next action>
2) <next action>
3) <next action>

## Resume Instructions (MUST WORK)
- Attach:
  - `tmux attach -t molt`
- Go to window/pane:
  - `<window name>`
- Resume command:
  - `<command>`
```

---

## Minimal “micro-report” (for frequent updates)
Use this when updating every 10–20 minutes.

```markdown
- [<HH:MM>] <status> — <one-line update>. Output: <path>. Next: <one-line next step>. Resume: <command>.
```

---

## AI Agent Rules
- Prefer writing reports to `logs/agents/<task>/report_<id>.md`
- Emit a micro-report to the terminal each checkpoint
- If you change files, always include paths and a one-line diff summary
- Never omit the resume command
