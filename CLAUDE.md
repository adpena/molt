# Claude Code Instructions

## ABSOLUTE NON-NEGOTIABLE: Zero Workarounds Policy

This is an early alpha project. We are the sole users and developers. There is ZERO tolerance for:

- **Workarounds** of any kind. If the correct fix requires refactoring, do the refactoring.
- **Hacky fixes**. No regex where structural parsing is needed. No bare except. No magic constants.
- **Partial fixes**. If a fix addresses 80% of cases, it's not done. Fix 100%.
- **TODO/FIXME as excuse to ship broken code**. If you write a TODO, implement it in the same turn.
- **"Simpler fix" that avoids the real problem**. The "simpler" path is always the workaround. Do the correct fix.
- **Technical debt**. We are building foundations. Every line of code must be defensible for the long term.
- **Code smell**. If something feels wrong, it is wrong. Fix it properly.
- **Silent failures or divergences from CPython >= 3.12**. Full deterministic parity except: no exec/eval/compile, no runtime monkeypatching, no unrestricted reflection.
- **Bypassing safety checks** (--no-verify, catch_unwind to swallow panics, etc.)
- **Sharp edges** left for "later". There is no later. Fix it now.

When you identify the correct fix and feel tempted to do something "simpler" instead — STOP. That temptation IS the signal that you're about to create a workaround. Do the correct fix.

## Engineering Standards

- **Correctness first, performance second, elegance third**. But all three are required.
- **NASA-grade quality**. Every change must be defensible as if deployed to production at scale.
- **Full parity** with CPython >= 3.12 for all supported features, including edge and corner cases.
- **All backends** (native/Cranelift, WASM, LLVM) must have parity. No backend-specific workarounds.
- **Extreme optimization and performance**. Choose the most performant algorithm and data structure. No lazy shortcuts.

## Git Discipline

- **NEVER revert or discard unstaged changes**. They are from trusted partners. Pause and wait.
- **NEVER trample partner work**. If you encounter unfinished changes, work around them or wait.
- **Always `git add` immediately** after writing any file. Linter hooks can silently revert unstaged changes.
- **Atomic operations**: write file + git add in the same step using `&&` chaining.

## Build & Test

- Build with `cargo build --profile release-fast -p molt-backend --features native-backend`
- Test with `python3 -m molt build --target native --output /tmp/test_out test_file.py --rebuild`
- Backend daemon uses release-fast profile. Kill with `pkill -9 -f "molt-backend"` before testing new builds.
- Max 2 build-triggering agents at once. 5 concurrent builds OOM the machine.

## Concurrent Development (MOLT_SESSION_ID)

`MOLT_SESSION_ID` **must be set BEFORE any build command**. Every agent must export it at the start of every shell command:

```bash
export MOLT_SESSION_ID="agent-1"  # MUST come before any molt or cargo command
```

Each session gets its own `target-<id>/` cargo directory (e.g., `target-agent_1/`). All cargo builds, path resolution, staleness checks, and cache lookups automatically route through the session-specific directory.

This gives each session:
- **Its own cargo target directory** (`target-agent_1/`) — no cargo lock contention, no artifact clobbering
- **Its own daemon socket** — no kill/restart conflicts between sessions
- **Its own build state and lock-check caches** — fully isolated build lifecycle
- **No `cargo clean`** — incremental builds only, no binary deletion

The first build in a new session takes approximately 5 minutes (full compile). Subsequent builds are incremental.

Without `MOLT_SESSION_ID`, all sessions share the default `target/` directory (solo dev mode).

Agents **MUST** use `export MOLT_SESSION_ID="unique-name"` at the start of every command to ensure isolation.
