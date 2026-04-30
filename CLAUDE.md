# Claude Code Instructions

## ABSOLUTE TOP PRIORITY (read this first, every turn)

**Engineer like Chris Lattner / Mojo / NASA. Never take a shortcut, workaround, or "simpler" implementation when the structurally correct fix is harder.** This rule overrides every comfort instinct.

The temptation chain you must reject:
- "I'll just promote Value-tier shadows at loop_start to fix this faster" → NO. That is a localized hack on top of an architecturally broken design. Do the structural redesign (typed IR, eliminate the shadow system) instead.
- "I'll add a small guard to handle this edge case" → NO. The edge case exists because the abstraction is wrong. Fix the abstraction.
- "I'll commit the partial fix and follow up later" → NO. There is no later. Either land the complete fix or do not start.
- "The full fix is too risky, let me ship something safer" → NO. The "safer" thing accumulates compound interest of bugs. Take the time. Do it right.
- "I'll skip the perf step and come back to it" → NO. Perf-correctness gaps create distrust. Land the fast version with the correct version.

When you identify a structurally correct fix and feel pulled toward an "immediate win" or "incremental approach", **STOP**. That pull IS the signal you are about to ship a workaround. Land the structural fix even if it is multi-day work.

If you cannot complete the structural fix in this session, **do not commit the localized hack as a placeholder**. Leave a clean baton-pass note describing the structural fix needed; the next session picks it up. Half-measures committed to main are worse than nothing committed.

This rule applies equally to:
- **Correctness**: bug class fixes, not bug instance fixes (e.g., fix the phi-representation invariant, not just the one site that exposed it)
- **Optimization**: structural codegen changes, not localized peephole tweaks
- **Performance**: redesign the hot path, do not add bypass cases
- **Architecture**: rework the abstraction, do not stack patches on a wrong foundation

Performance contract: molt MUST be faster than CPython on every benchmark, across every target (native, WASM, LLVM, Luau) and every profile (release-fast, dev-fast, debug-with-asserts). Do not declare a perf task complete until measurements confirm it on all targets.

## Top Priority: Tinygrad + DFlash Fidelity

This is a turn-blocking policy.

- Exact tinygrad semantics and API shape are the public ML contract. No drift is acceptable.
- Exact DFlash algorithmic fidelity is non-negotiable when implementing DFlash support. Do not ship generic speculative decoding under a DFlash label.
- `molt.gpu` and `molt.gpu.dflash` are implementation layers, not excuses to diverge from tinygrad or the DFlash paper/project.
- If the official DFlash design requires target-conditioned draft behavior, verifier/drafter separation, hidden-feature conditioning, KV injection, or a trained drafter, preserve those requirements. If a model lacks a real trained DFlash drafter, say so explicitly and do not fake support.
- If you detect existing drift from tinygrad or DFlash source-of-truth behavior, clean that drift up before adding more code.

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

## Bootstrap Authority (Non-Negotiable)

- Runtime-known module bootstrap must go through the runtime import boundary (`MODULE_IMPORT`). Do not split bootstrap ownership between frontend special cases and runtime import code.
- Bootstrap-critical builtin type objects such as `classmethod`, `staticmethod`, and `property` must come from explicit runtime bootstrap intrinsics/primitives. Do not probe-construct Python objects in stdlib bootstrap code to discover their types.
- When modifying `builtins.py`, `sys.py`, `importlib`, `_intrinsics.py`, or frontend import lowering, add or update native end-to-end bootstrap regressions in the same change.
- If a bootstrap fix depends on control-flow behavior in a fast-moving frontend/backend file, factor that dependency into a first-class runtime/bootstrap contract instead of leaving another chicken-and-egg edge in place.

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
- Max 3 backend daemons enforced by the CLI. Stale sockets are auto-cleaned.
- After a session with multiple agents, run: `pkill -9 -f "molt-backend" && rm -rf target-* .molt_cache_*`

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
