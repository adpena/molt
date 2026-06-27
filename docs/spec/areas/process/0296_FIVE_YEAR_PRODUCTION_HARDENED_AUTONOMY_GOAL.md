# Molt Five-Year Production-Hardened Autonomy Goal

Last updated: 2026-05-21

Absolute path:

```text
/Users/adpena/Projects/molt/docs/spec/areas/process/0296_FIVE_YEAR_PRODUCTION_HARDENED_AUTONOMY_GOAL.md
```

This document is a durable copy/paste prompt for autonomous Molt sessions. It is
an operating goal, not a replacement authority. `CLAUDE.md`, `AGENTS.md`,
`ROADMAP.md`, and `docs/spec/STATUS.md` remain authoritative and override this
prompt whenever they conflict.

## Optimum Copy/Paste Prompt

```text
You are working in /Users/adpena/Projects/molt, a Python AOT compiler.
Treat /Users/adpena/Projects/molt/docs/spec/areas/process/0296_FIVE_YEAR_PRODUCTION_HARDENED_AUTONOMY_GOAL.md as the durable five-year production-hardening goal for this session.

Run multi-hour autonomous loops. Default to a loop-style cadence, self-pace, and keep moving until a substantial structural batch is complete, a safety constraint is hit, or a real operator decision is required.

Authority chain, in priority order:

1. Read CLAUDE.md, AGENTS.md, ROADMAP.md, and docs/spec/STATUS.md at session start.
2. Re-read AGENTS.md before any commit.
3. Those four files override this prompt and any ad hoc instruction when they conflict.
4. Never invent a policy to resolve a conflict. If the authority chain is ambiguous, raise the exact ambiguity.

Non-negotiable engineering contract:

- Zero workarounds, hacks, partial fixes, TODOs-as-excuse, test-suite gaming, per-test special cases, silent host-CPython fallbacks, opt-in env-var gates for default behavior, --no-verify, or catch_unwind used to swallow panics.
- If a simpler local guard feels tempting, treat that temptation as evidence that the abstraction is wrong. Stop and repair the abstraction.
- Preserve global compiler coherence across frontend, typed IR, midend optimization, backend codegen, runtime, stdlib, tooling, and documentation.
- Prefer reusable compiler/runtime primitives over duplicated semantics.
- Every correctness, parity, compatibility, and performance claim must be backed by reproducible command output.
- Deterministic CPython >= 3.12 parity is required for the supported subset. Exclusions remain: exec/eval/compile, runtime monkeypatching, and unrestricted reflection.
- Native, WASM, LLVM, and Luau must converge on the same supported semantics.
- Tinygrad and DFlash fidelity are turn blockers. Do not fake DFlash with generic speculative decoding. Preserve target-conditioned draft behavior, verifier/drafter separation, hidden-feature conditioning, KV injection, and trained-drafter requirements.
- Never revert or discard partner work. Always start with git status and preserve unrelated WIP.
- Always stage owned file changes promptly so owned work is atomic and visible.

Maintainer/agent proof-lane environment setup before build, test, bench, or
molt commands:

export MOLT_SESSION_ID="<unique session id>"
eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"

On Windows checkouts on `C:`, heavy developer/agent lanes should resolve to a
healthy non-`C:` root unless an explicit emergency override is set. Public users
may compile in place, use Molt/Cargo defaults, or choose outputs with explicit
flags/environment variables.

Session startup:

1. cd /Users/adpena/Projects/molt
2. Read CLAUDE.md, AGENTS.md, ROADMAP.md, docs/spec/STATUS.md.
3. Run git status --short and identify partner WIP before touching files.
4. Run git log --since="7 days ago" --oneline | head -20 to orient.
5. Clean stale daemons and test processes only when safe: stale molt-backend daemons older than 1 hour, stale pytest processes older than 15 minutes, and stale canonical artifacts older than 1 day under target/sessions, build/wasm, and tmp. Never delete partner WIP.
6. Pick the highest-leverage in-flight or blocked Year-1 item that can be completed structurally now. Do not invent make-work.
7. Record the baseline gates before changing behavior.
8. Write a focused plan at /Users/adpena/Projects/molt/tmp/plan_<topic>.md with design, touched files, tests, exit criteria, and risk.
9. Execute, commit atomically when appropriate, rerun the smallest convincing proof matrix, and continue.

Required pre-commit and post-batch gates, unless the current authority files require a stricter matrix:

- cargo build --profile release-fast --workspace
- cargo test --profile release-fast -p molt-backend --features native-backend --lib
- python3 tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900; /Users/adpena/Projects/molt/.venv/bin/python3 -m pytest tests/compliance/ -p no:cacheprovider --tb=line -q
- git status --short

Gate rules:

- The build must have 0 errors and 0 warnings.
- Runtime/backend tests must pass without lowering the green count.
- Compliance must pass without lowering the green count.
- If a gate turns red after your change, fixing that gate becomes the next task.
- Add focused parity regressions for every implemented behavior across the affected backends.
- Refresh generated artifacts when their source of truth moves.
- Never repair these gates with raw PID/name/process-group cleanup. Stale Molt workers and daemons must go through the custody-aware sentinel or backend-daemon identity records so Codex, Claude, app-server, renderer, node-repl, shell hosts, and parent watcher processes remain untouchable.

Five-year production target:

Year 1, foundations:

- Complete the typed IR redesign and eliminate fast_int/raw_int/type_hint transport hints.
- Move sieve from roughly 13 ms toward 4 ms while ratcheting regressions such as class_hierarchy, struct, and bytes_find toward >= 1x CPython.
- Ratchet the 875 stdlib parity modules from intrinsic-partial to intrinsic-backed.
- Reach sanitizer-clean status across ASan, Miri, TSan, and UBSan.
- Establish real-hardware cross-platform matrix coverage for Raspberry Pi, Intel Mac, and Windows.

Year 1.5, ecosystem:

- Complete dlopen coverage for METH_VARARGS, METH_O, METH_KEYWORDS, and METH_FASTCALL.
- Unblock numpy, scipy, pandas, cryptography, lxml, pillow, ujson, pydantic-core, orjson, ruff, polars, and watchfiles lanes.
- Support advanced type-system cases used by attrs, pydantic v2, marshmallow, and cattrs.

Year 2, performance frontier:

- Implement short-string NaN-box inlining.
- Add rope/cord string concatenation.
- Integrate PGO/LTO.
- Build auto-vectorization for NEON and AVX.
- Add Perceus-style borrow inference for refcount elision.

Year 2-3, ML and AI production:

- Compile the openpilot policy model path from Comma.ai supercombo.onnx through tinygrad to ECU binary with real-car validation.
- Compile representative HuggingFace top-100 models including Whisper, LLaMA/Mistral 7B, BERT, ViT, and Stable Diffusion.
- Implement full DFlash algorithmic fidelity.
- Support edge AI deployment targets including Cloudflare Workers AI, AWS Lambda, Core ML, and NNAPI.

Year 3, systems platform:

- Build native Postgres, Redis, NATS, and Kafka lanes.
- Support FastAPI and Django compile lanes.
- Add OpenTelemetry, Dockerfile generation, and Kubernetes operator framework support.

Year 4, compiler frontier:

- Make the MLIR backend production-ready with polyhedral and affine optimization.
- Extend tools/z3_pass_verify.py into formal verification for TIR passes.
- Add translation validation.
- Support hardware specialization for ANE, Tensor Cores, Coral, GPU, and SPIR-V.

Year 5, ecosystem leadership:

- Replace CPython for AOT use cases in the supported subset.
- Self-host the compiler in Molt-compatible Python.
- Make browser Python real: DOM, WebGPU, <50 ms cold start, and <2 MB gzipped runtime.
- Explore language extensions such as gradual typing, effect systems, and compile-time evaluation when they improve the AOT contract.

Performance targets:

- Sieve vs CPython: 5x, then 25x, then 100x, then 1000x.
- Cold start: seconds, then <1 second, then <100 ms, then <50 ms.
- Binary size: roughly 15 MB, then <10 MB, then <5 MB, then <2 MB.

Portability targets:

- Current: macOS aarch64/x86_64 and WASM browser/Node/Cloudflare.
- Year 1: Linux aarch64/x86_64, Windows x86_64, iOS arm64, Android arm64.
- Year 3: FreeBSD and RISC-V Linux.

Known cross-runner context:

- Cross-runner: tools/cross_run.py using tools/cross_hosts.toml.
- macmini: 192.168.1.112, Intel Mac, rustup available, ssh available.
- molt-pi: 192.168.1.213, Raspberry Pi 5, Debian 13 trixie aarch64, ssh available, currently disk-full until the operator frees space.
- bat00: 192.168.1.216, Windows, no ssh:22 yet until the operator enables OpenSSH-Server.

Parallelism model:

- Use at most 3 parallel agents.
- Each agent must own a non-overlapping file scope.
- Coordinate through git status checks.
- Avoid cargo lock contention and backend daemon conflicts.

Reporting cadence:

- After each commit, report one line: commit SHA, what landed, and which gate it satisfies.
- After each batch of 3-10 commits, report gate outputs, recent commits, and working tree state.
- About once per hour, report what landed, what is in flight, what is blocked on the operator, and the next step.

Stop and escalate instead of retrying when:

- A public API decision is required.
- A safety constraint is hit.
- A test exposes ambiguous CPython compatibility policy.
- Remote capacity, usage caps, or agent caps block further progress.
- The only available next step would require deleting or overwriting uncommitted partner work.

When stopping:

- Leave a baton-pass note with what changed, what proof ran, what remains, what is blocked, and the exact next command or file to inspect.
- Ensure git status is explicit in the handoff.

Now go. Pick the highest-leverage structural work. Be ruthless about quality, ruthless about evidence, and ruthless about not inventing make-work.
```

## Minimal Invocation

For short restarts, paste this:

```text
Work in /Users/adpena/Projects/molt. Read CLAUDE.md, AGENTS.md, ROADMAP.md, docs/spec/STATUS.md, then execute the durable goal prompt at /Users/adpena/Projects/molt/docs/spec/areas/process/0296_FIVE_YEAR_PRODUCTION_HARDENED_AUTONOMY_GOAL.md. Pick the highest-leverage Year-1 structural item that can be completed now, preserve partner WIP, run the documented gates, and leave a baton-pass note when stopping.
```
