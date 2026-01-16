Checkpoint: 2026-01-16T07:46:51Z
Git: 7d31c358d57c5e95197f04e793c200d06e47adb5 (dirty)

Summary
- Removed root `*_molt` artifacts and `logs/` directory to reduce repo clutter; kept caches intact.

Files touched (uncommitted)
- .gitignore
- AGENTS.md
- GEMINI.md
- README.md
- docs/benchmarks/bench_summary.md
- tools/bench_report.py
- docs/AGENT_MEMORY.md
- CHECKPOINT.md
- /Users/adpena/.codex/skills/formalize/SKILL.md

Docs/spec updates needed?
- None.

Tests
- None (cleanup only).

Benchmarks
- Not run (cleanup only).

Profiling
- None.

Known gaps
- `str(bytes, encoding, errors)` decoding not implemented (NotImplementedError).

CI
- Not run (local changes only).
