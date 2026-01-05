# Contributing to Molt

Thank you for your interest in contributing to **Molt**.
Molt is a systems-level project: a compiler, runtime, and tooling stack designed for **performance, correctness, and long-horizon work**.

This document defines **how we work**, not just how to submit code.

---

## Core principles

Before contributing, understand these non-negotiables:

- **Long-running work is normal**
- **Persistent sessions are expected**
- **Progress must be observable**
- **Correctness beats cleverness**
- **Performance claims require evidence**

Molt is not optimized for drive-by patches. It is optimized for deep, durable engineering.

---

## Required setup (baseline)

Contributors are expected to have:

- `git`
- `tmux` (or equivalent)
- SSH access to their development machine
- A way to reconnect safely (mosh recommended)

See:
- `docs/REMOTE_ACCESS.md`

If you are using AI agents or Codex, this setup is **mandatory**.

---

## Working with tmux (required)

All long-running work MUST be done inside tmux.

Create or attach to the canonical session:

```bash
tmux attach -t molt || tmux new -s molt
```

Detach safely (leave work running):

```
Ctrl-b then d
```

If your work dies when you disconnect, it is considered incomplete.

---

## Agent and AI-assisted work

Molt explicitly supports AI-assisted development.

All AI agents MUST follow:
- `docs/AI_AGENT_OPERATING_ASSUMPTIONS.md`
- `docs/AGENT_PROGRESS_REPORT_FORMAT.md`

If you are prompting an AI agent, you must include the instructions from:
- `docs/AI_AGENT_INSTRUCTIONS_SNIPPET.md`

Failure to do so will result in rejected work.

---

## Starting a new task (required)

Every non-trivial change starts with a task directory.

Use the scaffold script:

```bash
tools/new-agent-task.sh <task-name>
```

This creates:

```
logs/agents/<task-name>/
├── report_<timestamp>.md
├── progress.log
└── artifacts/
```

All ongoing work must write progress here.

---

## Progress reporting (required)

### Micro-reports
During active work (every ~10–20 minutes):

- Append a one-line update to:
  ```
  logs/agents/<task>/progress.log
  ```

Example:
```
[14:32] running — implemented IR node validation. Next: add tests. Resume: tmux attach -t molt
```

### Full reports
Write a full report when:
- a logical phase completes
- files are modified
- a long command finishes
- work is paused or blocked

Use the canonical format:
- `docs/AGENT_PROGRESS_REPORT_FORMAT.md`

Never overwrite previous reports.

---

## Logging and benchmarks

Persistent sessions are only useful if output is durable.

All contributors MUST follow:
- `docs/LOGGING_AND_BENCHMARK_CONVENTIONS.md`

Key rules:
- Long-running commands must log to disk
- Benchmarks must be reproducible
- Results must not overwrite previous runs

Performance claims without benchmarks will be rejected.

---

## Code quality expectations

### Required
- Clear naming
- Deterministic behavior
- Explicit error handling
- Tests for non-trivial logic
- Comments where invariants matter

### Prohibited
- Hidden global state
- Silent fallbacks
- Performance claims without data
- “Temporary” hacks without tracking

---

## Commit and PR guidelines

### Commits
- Small, focused commits
- Descriptive messages
- Reference task name where applicable

### Pull Requests
PRs should include:
- What problem is being solved
- Why this approach was chosen
- Links to relevant agent reports
- Benchmark results (if performance-related)

PRs without context or evidence will be slowed or closed.

---

## What Molt is willing to break

Molt prioritizes:
- performance
- correctness
- explicitness

We are willing to break:
- undocumented behavior
- implicit assumptions
- legacy compatibility without justification

See:
- `docs/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md`

---

## Licensing

By contributing, you agree that your contributions are licensed under the project license (Apache 2.0 unless otherwise stated).

If you have concerns about licensing, raise them **before** submitting code.

---

## Final note

Molt is a serious systems project.
We value:
- patience
- rigor
- documentation
- evidence

If that excites you, you are in the right place.
