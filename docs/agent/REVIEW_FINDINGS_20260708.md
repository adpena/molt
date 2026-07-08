# Full-stack adversarial review findings — 2026-07-08

Source: `molt-fullstack-adversarial-review` workflow (37 agents, 9 dimensions,
adversarial verify). **26 CONFIRMED** findings (each independently refuted-then-
survived). These are ASSIGNED LANES: a Codex agent owns its lane's findings end-
to-end (fix + teeth + land + verify full surface). Do NOT freelance outside your
lane. Orchestrator owns build-throughput + coordinates the E1-adjacent ABI items.

| # | sev | lane | kind | file:line | finding → fix |
|---|-----|------|------|-----------|--------------|
| 1 | P0 | CODEX-ABI (E1-adjacent; coordinate w/ orchestrator) | bug-class | `runtime/molt-cpython-abi/src/api/typeobj.rs:463` | **PyType_FromMetaclass/FromSpec Rust exports ignore spec->slots and fail open (mark type READY with zero behavior)** — Delete the Rust FromSpec-family #[no_mangle] stubs so the header static-inline (which processes every slot and calls the runtime type() authority) is the single authority; add PyType_FromMetaclass to include/molt/Python.h as a static-inline… |
| 2 | P1 | ORCH (build-throughput) | optimization | `.cargo/config.toml:8` | **Windows-msvc cargo builds use slow link.exe; no fast-linker config despite rust-lld shipping in the pinned toolchain** — Add `[target.x86_64-pc-windows-msvc]\nrustflags = ["-C", "linker-features=+lld"]` (version-safe: the flag is stable in the pinned 1.96.1 toolchain and uses the bundled rust-lld, needing nothing on PATH). SAFE with panic=unwind/catch_unwind:… |
| 3 | P1 | CODEX-PERF | metabug | `runtime/molt-backend-native/src/native_backend/function_compiler/fc/list_index_fast_path.rs:134` | **Loop-invariant list data_ptr/len hoist ignores opaque-call and alias mutation** — Treat any op that can pass the list to opaque code (call/call_method/call_function/store into an escaping container, and any aliasing copy_var whose target is later mutated) as a mutation in scan_loop_hoistable_lists, OR gate hoisting on a … |
| 4 | P1 | CODEX-FRONTEND | bug-class | `src/molt/frontend/lowering/analysis_collect_static.py:486` | **Non-comprehension walrus (:=) targets are omitted from _collect_assigned_names, corrupting scope_assigned / unbound-checks / closure-cell boxing / free-var classification** — Add a `visit_NamedExpr` handler to the `_collect_assigned_names` collector (lines 489-596) that adds `node.target.id` when the target is an `ast.Name` and then `generic_visit(node.value)`, exactly mirroring `_collect_assigned_names_ordered`… |
| 5 | P1 | CODEX-CORRECTNESS | bug-class | `runtime/molt-passes/src/tir/passes/sccp.rs:799` | **SCCP folds str builtins/methods with Rust byte/UTF-8 semantics, silently miscompiling non-ASCII string constants** — Route SCCP str concrete-eval through CPython code-point semantics: len via `s.chars().count()`; find/rfind must translate the byte offset returned by `str::find` into a char index (count chars before the byte offset) or return None to defer… |
| 6 | P1 | CODEX-WASM | bug-class | `runtime/molt-backend-wasm/src/wasm/function_frame/planning.rs:253` | **WASM tail calls (return_call/0x12) emitted unconditionally, never feature-gated** — Add a `tail_call_enabled` field to WasmCompileOptions (default derived from the target profile / MOLT_WASM_* env like native_eh_enabled), thread it into CallOpContext, and require it in is_tail_call_candidate so `return_call` is only emitte… |
| 7 | P1 | CODEX-METABUG/DX | metabug | `src/molt/cli/frontend_worker.py:889` | **MOLT_FRONTEND_PHASE_TIMEOUT silently no-ops on Windows and in worker threads (configured != effective)** — Provide a portable watchdog: spawn a daemon threading.Timer that interrupts/raises (or sets a cooperative cancel flag polled by the lowering loop) on platforms/threads without SIGALRM, so the configured bound is enforced everywhere. At mini… |
| 8 | P1 | CODEX-METABUG/DX | metabug | `src/molt/cli/frontend_parallel.py:385` | **Configuring MOLT_FRONTEND_PHASE_TIMEOUT silently forces fully-serial frontend lowering** — Decouple the safety knob from the parallelism decision. Enforce a per-worker/per-module timeout inside each parallel worker (a portable watchdog per task) instead of requiring a single-process SIGALRM, so the pool stays enabled with a timeo… |
| 9 | P1 | CODEX-METABUG/DX | metabug | `tools/parity_gate.py:316` | **Parity gate downgrades real Molt regressions to SKIP on any ImportError-shaped stderr (fail-open on the merge-blocking STRICT gate)** — Only skip when CPython ALSO failed on the same import (both sides import-failed), or require an explicit per-file `# molt-parity: excluded` marker; never infer skip from a Molt-only stderr substring on a STRICT test. |
| 10 | P1 | CODEX-METABUG/DX | metabug | `tools/ci_gate.py:1267` | **ci_gate exits 0 when required checks are SKIPPED for a missing toolchain (vacuous green)** — Split skip semantics: a missing prerequisite for a `required=True` check must fail the gate (or emit a distinct 'unmet-prerequisite' terminal status that maps to non-zero exit), reserving benign skips for genuinely optional checks. |
| 11 | P2 | ORCH (build-throughput) | optimization | `Cargo.toml:347` | **release-fast emits debuginfo (debug=1 inherited) that strip="symbols" then discards — paying rustc debuginfo + Windows PDB link cost on the build-iteration long pole** — Set `debug = 0` on [profile.release-fast] (and reconsider it on [profile.release]). This drops rustc debuginfo emission and eliminates the .pdb generation from the link.exe step, directly cutting the Windows link long pole. If daemon-panic … |
| 12 | P2 | ORCH (build-throughput) | optimization | `runtime/molt-runtime/Cargo.toml:246` | **aws-lc-sys (heavy C+asm, NASM-dependent on Windows) is compiled into the DEFAULT native runtime staticlib via rustls' default aws-lc-rs provider** — Build rustls with `default-features = false` and the lighter `ring` provider (`features = ["ring", "tls12", "logging"]`) and give tungstenite a matching ring-based tls feature, OR gate the crypto-provider selection so aws-lc only compiles w… |
| 13 | P2 | CODEX-PERF | optimization | `runtime/molt-runtime/src/object/ops/specialized_list.rs:441` | **GIL-entry (with_gil_entry_nopanic) on primitive list element fast paths — inconsistent with getitem_int_fast** — Restructure each to do the pure NaN-box work without a GilGuard and enter the GIL only on the rare allocating branch (float_result_bits NaN case, or promotion). For molt_list_float_getitem, inline the value.is_nan() check and only call the … |
| 14 | P2 | CODEX-PERF | optimization | `runtime/molt-runtime/src/object/ops/specialized_list.rs:24` | **Specialized-list slice double-allocates (temp Vec then copy) and loses flat specialization** — Add a builder that allocates the destination list backing store up front and fills it in place from the source slice (for the flat cases, allocate the corresponding ListIntStorage/ListFloatStorage/ListBoolStorage and copy raw elements, pres… |
| 15 | P2 | CODEX-FRONTEND | optimization | `src/molt/frontend/lowering/analysis_collect_static.py:965` | **Free-variable analysis re-walks every nested function subtree once per ancestor plus once per compile — O(N*D) redundant full-AST traversals with no memoization** — Memoize free-var results per AST node (id(node) -> frozenset) so a nested function is analyzed once and reused by both its ancestors' `_collect_nested_free_vars` and its own compile pass; optionally fuse the multiple independent per-body co… |
| 16 | P2 | CODEX-CORRECTNESS | bug-class | `runtime/molt-passes/src/tir/passes/sccp.rs:854` | **SCCP str()/repr() folding of float and repr(str) diverge from CPython formatting** — Refuse to fold str/repr of floats whose CPython formatting uses scientific notation (or reproduce CPython's `repr` float algorithm / shortest-repr-with-exponent-threshold), and implement CPython's repr(str) quote-selection (prefer single, s… |
| 17 | P2 | CODEX-CORRECTNESS | metabug | `runtime/molt-backend-native/src/native_backend/function_compiler/scalar_carriers.rs:319` | **Raw sdiv/srem divide lane correctness depends on two unasserted non-local invariants (INT_MIN/-1 signed-overflow)** — At the raw-lane emit site, mirror the explicit divide-by-zero handling with an explicit divisor==-1 special-case: for floordiv emit result = 0 - dividend on the -1 branch (Python semantics, with the boxed slow path for the INT_MIN overflow)… |
| 18 | P2 | CODEX-WASM | optimization | `runtime/molt-backend-wasm/src/wasm_data.rs:117` | **Data-segment start alignment uses previous segment's alignment mask (off-by-one)** — Align the CURRENT segment's start to its own requirement before placing it: compute `align_mask` from this segment's byte_len first, do `offset = (self.offset + align_mask) & !align_mask`, emit at that offset, then advance self.offset by by… |
| 19 | P2 | CODEX-WASM | metabug | `runtime/molt-backend-wasm/src/wasm/module_abi/finalize.rs:152` | **No target_features custom section emitted, so required proposals (tail-call, EH) are undiscoverable by tooling** — Emit a `target_features` custom section reflecting the features actually used this compile: set `tail-call` when tail_calls_emitted>0 and `exception-handling` when native EH tags were emitted (finalize.rs already tracks both: tail_calls_emi… |
| 20 | P2 | CODEX-ABI (E1-adjacent; coordinate w/ orchestrator) | metabug | `runtime/molt-cpython-abi/src/api/typeobj.rs:184` | **PyType_Ready drops methods fail-open while getset/members fail-closed on the same dict-store failure** — Make the method store-failure path fail closed to match members/getset: on PyDict_SetItemString rc<0, set an honest exception if none pending and return -1 from add_methods_to_dict, so a degraded dict backend cannot silently ship a half-pop… |
| 21 | P2 | CODEX-ABI (E1-adjacent; coordinate w/ orchestrator) | metabug | `runtime/molt-cpython-abi/src/api/imports.rs:106` | **Header static-inline vs Rust #[no_mangle] divergence: PyImport_GetModuleDict returns a private empty dict instead of live sys.modules** — Establish one authority per ABI symbol: delete the Rust PyImport_GetModuleDict body in favor of the header inline (or make the Rust export delegate to the same live-sys.modules path), and add a drift gate that fails when a cpython-abi #[no_… |
| 22 | P2 | CODEX-METABUG/DX | metabug | `runtime/molt-runtime/src/async_rt/scheduler/compile_governor.rs:48` | **Self-healing compile governor (MOL-213) is entirely dead code; its env knobs have zero effect** — Either wire the governor into the compilation task admission/execution path (call try_admit before admitting a compile task, check_budget after, thread OptLevel into the pass pipeline, and surface status_snapshot) so the knobs and telemetry… |
| 23 | P2 | CODEX-METABUG/DX | metabug | `src/molt/cli/runtime_wasm_cache.py:140` | **Shared runtime-wasm cache publish failures are swallowed with no telemetry, causing silent cold-every-session** — Count publish failures and surface them (build warning + a hit/publish-rate field in build diagnostics) so a persistently-failing cache is a loud DX defect, matching the backend_cache.py behavior and the project instruction to treat publish… |
| 24 | P2 | CODEX-METABUG/DX | optimization | `runtime/molt-backend-native/src/llvm_backend/mod.rs:116` | **Polly polyhedral optimization can be silently absent with no attestation that it ran** — Probe/verify Polly availability at init (e.g. query a Polly-registered pass or cl::opt) and record an attestation flag in build diagnostics; fail loudly or emit a warning when the `polly` feature is compiled but the host LLVM ignored the fl… |
| 25 | P2 | CODEX-METABUG/DX | bug-class | `tools/ci_gate.py:651` | **ci_gate script-existence skip guard indexes the wrong argv element and is dead code** — Index the actual script token (scan cmd after the interpreter for the first arg under TOOLS, e.g. cmd[5] for _uv_run) or resolve the script positionally from the builder, and cover it with a test that a missing script yields the skip. |
| 26 | P2 | CODEX-METABUG/DX | metabug | `tools/check_perf_gate_wiring.py:60` | **perf-gate-wiring audit certifies the gate 'fires' without checking it is blocking (continue-on-error / always-false if blind spot)** — Parse the scoreboard step and assert it has no `continue-on-error: true`, no trivially-false `if:`, and lives in a job that is required/blocking on the main/PR path; fail closed if the invoking step is non-blocking. |

## Landing status (orchestrator)
- **#2 rust-lld linker: LANDED** (`858c6a306`). The review's "`-C linker-features=+lld`
  is stable" claim was WRONG (unstable on 1.96.1 — verify build failed). Correct
  stable+portable fix: `_maybe_enable_lld_link` auto-detects LLVM `lld-link` and
  sets `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER` (env, non-RUSTFLAGS; no-op where
  absent). Verified: lld-link LINKS the daemon (queue LINK_OK). 4 teeth.
- **#4 walrus scope-analysis: LANDED** (`8883a352c`). `_collect_assigned_names` now
  collects `NamedExpr` targets (was dropping them; nested scopes don't leak).
- **#9 parity gate Molt-only ImportError fail-closed: LANDED** (`e00d69a98d`).
  STRICT Molt-only import failures now fail instead of skipping; only a same
  import failure on both CPython and Molt downgrades to skip. Re-verified
  2026-07-08 with `pytest tests/tools/test_parity_gate.py -q` (`5 passed`).
- **#10/#25 ci_gate skip/prerequisite fail-closed: LANDED**. `ci_gate` now
  distinguishes optional skips from required unmet prerequisites, includes
  `unmet-prerequisite` in JSON success/summary and required-failure exit logic,
  and scans command argv for the first `tools/*.py` script before execution.
  The proof also fixed the DX tests to mark intentional `D:\Molt` fallback roots
  with `MOLT_PRESERVE_LEGACY_ARTIFACT_ROOTS=1` while preserving the stale-root
  scrubber tooth, and prevents auto-janitor orphaning during pytest runs.
  Verified with `uv run --active --project . --python 3.12 pytest
  tests/tools/test_ci_gate.py tests/test_dx_run_context.py -q` (`59 passed`).
- **#11 release-fast debug=0: LANDED** (`f21cf71aa`).
- **#13 specialized-list primitive GIL fast paths: LANDED**. Regular
  `STORE_SUBSCR_LIST_INT`, raw-index list store, and unchecked list getitem now
  share an explicit primitive-vs-heap-ref gate: inline primitives bypass
  `with_gil_entry_nopanic`, while heap-ref updates still enter the canonical
  refcount path. Verified with `cargo test -p molt-runtime specialized_list
  --lib` (`2 passed`, `484 filtered out`).
- **#14 specialized-list slice flat builder: LANDED**. Specialized int/bool
  slicing now preserves `TYPE_ID_LIST_INT`/`TYPE_ID_LIST_BOOL` and fills flat
  storage directly through the shared builder authority; list copy, list repeat,
  and `molt_list_int_new` use the same specialized-list allocation primitive
  instead of hand-allocating storage/object pairs. Verified with `cargo test -p
  molt-runtime specialized_list --lib` (`5 passed`, `484 filtered out`).
- **#16 SCCP float/repr constant-fold parity: LANDED** (`af7fe19820`). The
  single SCCP concrete-eval authority now folds `str()`/`repr()` of floats only
  inside the finite non-scientific CPython/Rust-agreeing regime and defers
  exponent/non-finite values to the runtime formatter; `repr(str)` folds only
  byte-for-byte safe printable ASCII and defers quote/escape cases. Re-verified
  on the current tree with proof-queue run
  `20260708T225511-review16-molt-passes-lib-84f936bef4a14683`
  (`cargo test -p molt-passes --lib`: `841 passed`).
- **#18 WASM data segment alignment: LANDED**. Each segment now aligns its own
  start before emission instead of inheriting the previous segment's alignment;
  re-verified with `cargo test -p molt-backend-wasm --features test-util
  wasm_data::tests` (`2 passed`) and `cargo test -p molt-backend --features
  wasm-backend --test wasm_data_segments` (`9 passed`).
- **#23 runtime-wasm shared cache publish telemetry: LANDED**. Runtime wasm
  shared-cache publish failures now increment process-local cache diagnostics,
  retain the last failure reason, warn on human build output, and flow into
  `build_diagnostics.runtime_wasm_cache` so a broken shared cache is visible
  instead of degenerating into silent cold rebuilds. Re-verified with
  `tests/cli/test_cli_runtime_wasm_shared_cache.py` plus the four
  runtime-wasm-cache diagnostics tests in
  `tests/cli/test_cli_import_collection.py` (`15 passed`).
- NOTE: not in the review but landed same arc — the biggest build-throughput win was
  `ad0cafb82` **adaptive cargo jobs (2→14)**: a hardcoded CARGO_BUILD_JOBS=2 defeated
  the memory-bounded ceiling (~7x under-parallelism). Plus `bdd42535e` persistent
  target dir + `aa15340aa` incremental-when-sccache-off.
- All others: OPEN — Codex lanes claim via docs/agent/CLAIMS.md, land per the NEW PROTOCOL.
  Highest-value OPEN: #1 (P0 PyType_FromMetaclass fail-open, E1-critical),
  #7/#8 (frontend-timeout → serial degradation, witness-throughput),
  #1/#7/#8 remain the next high-value fail-open/vacuous-green risks.
