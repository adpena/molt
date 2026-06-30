<!-- Foundation design 31. Architect: read-only research-granted agent, 2026-06-06.
Saved verbatim. Key audit finding: tools/fuzz_compiler.py (2,855 lines, 3 modes) +
libFuzzer targets + 62 Kani harnesses ALREADY EXIST — this design extends that
substrate (generator feature families, 5-oracle stack incl. the CPython-free
backend-divergence oracle, AST-aware reducer, corpus ratchet, CI tiers) rather than
duplicating it. The would-have-caught table (Part 1.5) is the acceptance calibration:
the current generator misses ALL of this week's bug classes (walrus, {expr=}, BigInt
boundaries, nonlocal-augassign, membership variants). -->

# Molt Generative Differential-Fuzzing Foundation — Architecture Blueprint

## Design Provenance

**Document**: `docs/design/foundation/31_differential-fuzzing-lane.md`
**Status**: OUTSTANDING (no committed lane exists)
**Composes with**: sanitizers.yml (ASan+Miri weekly), kani.yml (formal), nightly.yml (differential-basic-stdlib), fuzz/ (libFuzzer corpus)
**Research citations**: YARPGen [Livinskii et al., OOPSLA 2020]; CSmith [Yang et al., PLDI 2011]; hypothesmith [Zac-HD/hypothesmith]; Jit-Picking [Bernhard et al., CCS 2022]; Ratte [ASPLOS 2025]; Hypothesis shrinking internals; cargo-fuzz/arbitrary

---

## Part 1 — Existing Infrastructure Audit

### 1.1 Oracle machinery

**`tests/molt_diff.py`** is the differential runner. It is a self-bootstrapping harness (~2900 lines) that drives CPython + molt compile + molt run, compares stdout byte-exactly, and handles the following concerns that the fuzzer must inherit:

- **Version-gated stderr comparison**: `_stderr_matches` at molt_diff.py:377 dispatches on `exception_signature` mode to compare only exception type+message, ignoring frame formatting. This is the correct oracle for stderr — the fuzzer must use the same logic, not raw equality.
- **`# MOLT_META:` annotations**: per-file `skip=`, `platforms=`, `py=`, `min_py=`, `max_py=`, `wasm=`, `expect_fail=` fields parsed at molt_diff.py:159 control gating. Generated programs have no metadata, so the fuzzer skips all gate logic and runs everything.
- **`# MOLT_ENV:` annotations**: per-file env overrides. Generated programs never emit these — the fuzzer uses a fixed deterministic env (PYTHONHASHSEED=0, MOLT_DETERMINISTIC=1).
- **`_molt_sys_env_for_python_exe`** at molt_diff.py:251: derives MOLT_PYTHON_VERSION + MOLT_SYS_VERSION_INFO from the CPython baseline being run. The fuzzer must call this path when using a non-default CPython (3.13, 3.14) so molt's version-gated messages align.
- **`MOLT_DIFF_RESULTS_JSONL`** (molt_diff.py:519): per-test JSONL sink for the suite-honesty ratchet (#46). Fuzzer must write to this file so every generated failure is visible to `tools/check_suite_honesty.py`.
- **`BatchCompileServerClient`** (tools/batch_compile_client.py): optional batch-compile server; the fuzzer should support this path but not require it, because fuzz programs are single-shot and not cached usefully.

**`tools/fuzz_compiler.py`** (2855 lines) is an existing generator+runner with three modes: `safe` (type-tracked AST generator), `reject` (dynamic-feature programs), `compile-only` (hypothesmith grammar-based). Key observations:

- The `SafeProgramGenerator` class at fuzz_compiler.py:178 is a complete type-tracked statement generator covering ~30 feature families, with closure/class/inheritance/kwonly/*args generation, scope tracking, and deterministic output via sorted() wrappers.
- **Critical gap**: the generator is entirely missing molt's known high-risk feature families: walrus in comprehensions, f-string `{expr=}` debug format, augmented-assign chains across closure boundaries, generator shapes, match/case, async for/with, exception groups, BigInt boundary arithmetic (2^46, 2^60, 2^63), cross-inlining scenarios (calls into module-scope closures), descriptor protocol chains.
- The shrinking at fuzz_compiler.py:2190 implements block-level delta-debug (top-level statement removal, iterating to fixpoint). This is structurally correct but shallow — it cannot reduce inside a function body or simplify expressions.
- **No coverage signal**: the runner collects pass/mismatch/timeout/build_error counts but never queries TIR_OPT_STATS or any pass-fire signal. There is no feedback loop from pass activation to generation bias.
- **No backend-divergence oracle**: the runner only compares against CPython. It never compiles the same program to WASM or LLVM and cross-checks the backends against each other, which is molt-specific and requires zero CPython runs.
- The `CompileOnlyFuzzer` at fuzz_compiler.py:1660 uses hypothesmith's `from_grammar()` via `hypothesis.find()` but does not execute the programs — it only checks molt does not crash. This is correct for grammar-fuzzing but misses the differential signal.
- **No corpus management**: failures are saved to `--output-dir` but there is no automatic promotion to `tests/differential/basic/` and no deduplication ledger.
- **No CI wiring**: fuzz_compiler.py is a standalone script invoked manually; it has no nightly.yml job.

**`tools/safe_run.py`**: provides the RSS+wall-time watchdog. The fuzzer must route all binary execution through it (or through the equivalent `harness_memory_guard.guarded_completed_process` which `fuzz_compiler.py` already uses for CPython runs). `safe_run.py` exit 124 = TIMEOUT, 137 = OOM — these map to the fuzzer's `timeout` / `oom` failure categories.

**`MOLT_ASSERT_NO_LEAK`**: when set in the env, the runtime's `assert_no_leak_at_exit` (runtime/molt-runtime/src/object/ops.rs:762) aborts with exit 137 if `live_objects > EXPECTED_LIVE_OBJECTS`. The fuzzer can set this flag on every run and treat non-zero exits as a third oracle dimension (memory leak). The expected-live floor is a compile-time constant, making this oracle deterministic.

**`TIR_OPT_STATS`**: env var read at runtime/molt-backend/src/wasm.rs:2094 and runtime/molt-backend/src/main.rs:49 (both declared in `DAEMON_REQUEST_ENV_KEYS`). When set to `1`, the backend emits per-function pass-fire stats to stderr. This is the proxy coverage signal available without instrumented builds.

### 1.2 Fuzz crates

`fuzz/Cargo.toml` and `runtime/molt-backend/fuzz/Cargo.toml` each have libFuzzer targets:

- `fuzz_nan_boxing.rs`: exercises all NaN-box encode/decode paths with `ValueInput` enum. Comprehensive and correct — covers the 47-bit int boundary directly.
- `fuzz_ir_parse.rs`: feeds arbitrary bytes to `SimpleIR::from_json_str`. Correct no-panic contract.
- `fuzz_ir_passes.rs`: generates random SimpleIR and runs the full pass pipeline. Key gap: generates only the old SimpleIR pass set (`fold_constants`, `rc_coalescing`, etc.) — it does NOT exercise the new TIR pass pipeline (`run_pipeline` from `pass_manager.rs`). The TIR pipeline is where SCCP, BCE, loop transforms, alias analysis (S5), and the inliner run.
- `fuzz_wasm_compile.rs`: generates random SimpleIR and feeds it to the WASM backend. Same gap: generates SimpleIR, not TirFunction — the TIR-level optimizations are bypassed.
- `runtime/molt-backend/fuzz/fuzz_tir_passes.rs`: generates random `TirFunction` with the new block/op structure and calls `passes::run_pipeline`. This is the correct TIR fuzzer and exercises the right surface. However: (a) it calls `std::panic::catch_unwind` and `resume_unwind` to register panics — this is the correct pattern; (b) the `OPCODES` palette (fuzz_tir_passes.rs:22) omits several TIR opcodes that are load-bearing for correctness: `CheckException`, `TryStart`, `TryEnd`, `StateBlockStart`, `StateBlockEnd`, `IterNext`, `GetAttr`, `SetAttr`, `Call`, `ModuleImport`, `ModuleImportFrom` — i.e. the entire exception-augmented CFG and object-model surface.
- `runtime/molt-runtime/fuzz/Cargo.toml`: has `string_ops.rs` and `fuzz_vfs_caps.rs` targets (runtime C-ABI surface).

**No fuzz target exists for**: the native Cranelift backend (`compile_function` in `function_compiler.rs`), the LLVM lowering pass (`llvm/lowering.rs`), the TIR-to-SimpleIR lowering (`lower_to_simple.rs`), or the frontend itself (AST → TIR).

### 1.3 Kani proofs

62 harnesses across 4 files (kani_nanbox.rs, kani_refcount.rs, kani_object.rs, kani_string_ops.rs) covering NaN-box algebra, refcount arithmetic, header layout, and string slice safety. These are correct and comprehensive for what they cover. **Not yet covered**: `dec_ref_ptr` full deallocation path, concurrent refcount, wasm32 Cell<u32> path, pointer round-trip through registry. These are documented as known gaps in `runtime/molt-obj-model/KANI.md`.

### 1.4 SIGURG and harness hazards

From `docs/design/foundation/22_bughunt_wave1.md:295`: "long harness/cargo runs reliably receive SIGURG (exit 143/144) on this host — keep batches short." This is a macOS-specific signal delivered by the OS when a socket receives out-of-band data or when certain timing events occur with background daemon I/O. The differential runner's daemon (backend daemon) uses tokio sockets; a long-running pytest session that holds daemon connections across many test files accumulates SIGURG risk. The SIGURG-immune architecture is: run fuzz in small batches of ≤50 programs per subprocess, re-exec between batches, collect results via JSONL (append-safe), and never hold daemon connections across batch boundaries. This matches the `re-exec` pattern documented in the wave-1 baton.

### 1.5 What the existing generator WOULD NOT have caught

The five bug classes from the session preamble, mapped against the existing `SafeProgramGenerator`:

| Bug class | Generator coverage? | Why missed |
|---|---|---|
| default-miss op routing (5 instances, e.g. `ModuleImportFrom`) | No | Generator never emits `from X import Y` at module scope in combinations that trigger the new opcode routing |
| membership-family miscompiles (`x in set`, `x in dict`) | Partial | Generator emits `in` but only on list literals, not on generated sets/dicts with non-trivial keys |
| comp-walrus storage | No | Walrus (`x := expr`) is completely absent from `SafeProgramGenerator` |
| closure env-misbind | Partial | Closure generator produces only three shapes (simple/counter/accumulator), not the problematic cross-scope augmented-assign variants |
| f-string `{expr=}` multi-site | No | `_gen_fstring_inner` at fuzz_compiler.py:471 never generates `{expr=}` syntax |
| batch-boundary sensitivities | No | No batch-compile mode; every program is a fresh single-file build |
| BigInt 2^46/2^60/2^63 boundaries | No | `INT_RANGE = (-200, 200)` at fuzz_compiler.py:200 |

This confirms the audit: the existing generator has the right architecture but incorrect generation weights and critical feature omissions.

---

## Part 2 — Architecture Decision

**Chosen approach**: extend and harden `tools/fuzz_compiler.py` as the authoritative differential fuzzer rather than building a separate tool. The existing runner infrastructure — CPython execution, molt build/run, harness_memory_guard, JSONL recording, shrinking scaffold — is structurally correct and already wired to the right executor paths. Building a new tool would duplicate this and introduce divergence. The structural fix is to replace the generator's feature set, add the missing oracles, wire coverage feedback, add the backend-divergence oracle, extend the Rust fuzz targets, and add CI jobs.

This is a single complete arc: the fuzzer is not useful until all five oracles are active and the generator covers the known risk surface. Partial landing (generator without coverage feedback, or oracles without CI) is a partial implementation per the CLAUDE.md policy. The phases below are designed to each be a complete, independently verifiable unit.

**Why not hypothesmith-first**: `from_grammar()` generates syntactically valid but semantically arbitrary Python. For molt's use case this yields ~70-80% CPython-error programs (undefined behavior, unbound names, type errors) that consume build budget without producing differential signal. The type-tracked generator approach (already in `SafeProgramGenerator`) is the right design — it just needs the right feature weights. hypothesmith is retained for the compile-only crash-testing lane only.

---

## Part 3 — Component Design

### 3.1 Program generator: `tools/fuzz_generator.py` (new module, extracted + extended)

**File**: `/Users/adpena/Projects/molt/tools/fuzz_generator.py`

The `SafeProgramGenerator` at fuzz_compiler.py:178 is extracted into a standalone module and extended. The extraction is mandatory because fuzz_compiler.py is 2855 lines; the generator is the largest independent unit and must be testable in isolation without running the full fuzzer loop.

**New feature families to add (by bug-class priority)**:

1. **Walrus operator** (`WalrusGenerator`): generates `:=` in comprehension filters (`[x for x in lst if (y := f(x)) > 0]`), in while conditions, in nested comprehension targets. These are the patterns that exposed the comp-walrus storage bug class. Scope rules: walrus in comprehension body assigns to the enclosing function scope, not the comprehension scope — the generator must track this correctly.

2. **F-string `{expr=}` debug syntax** (`FstringDebugGenerator`): generates `f"{expr=}"` and `f"{expr!r}"` for arbitrary typed expressions. The `!r`/`!s`/`!a` conversions plus `=` suffix hit distinct codegen paths. Multi-site generation (multiple `{expr=}` in one f-string) is the bug-class shape.

3. **BigInt boundary arithmetic**: extend `INT_RANGE` to include deliberate boundary values: `2**46 - 1`, `2**46`, `2**46 + 1`, `2**60 - 1`, `2**60`, `2**63 - 1`, `2**63`, `-2**63`. These are the RawI64Safe/MaybeBigInt boundary transitions that the ValueRange substrate (S6) was built to handle. Generator emits these as integer literals, not runtime expressions, so CPython always computes them correctly.

4. **Augmented assignment across closure scope**: generates `nonlocal x; x += expr` patterns where x is initialized in an outer scope. This is the closure env-misbind class. The current closure generator only uses list cells (`captured[0] += val`), not `nonlocal`.

5. **Generator shapes**: generates `def gen(): yield expr` with `next()`, `list()`, `for x in gen()`, generator expressions passed to `sum()/list()/tuple()`. The generator fusion design doc (docs/design/generator_fusion.md) identifies these as a primary risk surface for the deforestation/iter_devirt interaction.

6. **match/case statements**: generates structural `match obj: case Type(field=val)` patterns for class instances generated by the existing class generator. This exercises the pattern compiler.

7. **Exception group and `except*`**: generates `ExceptionGroup(msg, [exc1, exc2])` and `try: ... except* ValueError as eg:` shapes. These are 3.11+ syntax that molt must support.

8. **Cross-scope membership**: generates `x in s` where `s` is a dict, a set with generated keys, or a frozenset — not just list literals. The membership-family miscompile was on dict and set variants.

9. **Module-scope from-import**: generates `from module import name` at module scope (where the `ModuleImportFrom` opcode fires), including re-export patterns and aliased imports.

10. **`{expr!r}` vs `repr(expr)` equivalence**: these must produce identical output; the fuzzer explicitly generates both forms with the same expression and asserts they match. This is a molt-specific property test, not a CPython comparison.

**Generation policy** (following YARPGen's generation-policies insight): rather than a fixed probability table, the generator maintains a `FeaturePolicy` struct with per-family weights. The default policy has equal weights. A `RiskBiasedPolicy` tilts weights toward the 10 high-risk families above (3x weight each). A nightly run uses `RiskBiasedPolicy`; a PR smoke run uses the default. This is how YARPGen avoids the saturation problem: biased policies generate programs that exercise more optimization-triggering patterns.

**Coverage proxy signal** (`TIR_OPT_STATS`): after each molt build, if `TIR_OPT_STATS=1` is set, parse the per-function pass stats from stderr. Track a `PassFireCounts` dict mapping pass name → fire count across the batch. A program is "interesting" (higher weight for corpus retention) if it fires passes that the current batch has fired infrequently. This is a soft coverage signal, not a hard coverage-guided loop like AFL. It is correct to implement it as a post-hoc filter rather than as a generator oracle because the generator's semantic constraints already produce valid programs — coverage only determines *which* valid programs to keep.

**Determinism**: the generator is seeded with `Random(seed)`. Given the same seed, the same program is always produced. Seeds are stored in the JSONL record. Corpus files stored in `tests/differential/basic/` get the seed embedded as `# MOLT_FUZZ_SEED: N` comment so they can be regenerated and bisected.

### 3.2 Oracle stack: `tools/fuzz_oracles.py` (new module)

**File**: `/Users/adpena/Projects/molt/tools/fuzz_oracles.py`

Five oracles, composable, all must pass for a program to be considered correct.

**Oracle 1 — CPython differential (stdout byte-identical)**:

The existing logic in `fuzz_compiler.py:1986` (`cp_stdout == molt_stdout`) is correct but incomplete. The complete oracle is:

- stdout: byte-identical (same as now)
- exit code: if CPython exits 0, molt must exit 0; if CPython exits nonzero (exception), molt exit code is not required to match exactly (CPython uses 1 for unhandled exceptions, molt may differ slightly on exception-path exits), but molt must not exit 0 when CPython exits nonzero
- stderr: exception-type and message match using the `_extract_exception_signature` logic already in molt_diff.py:365 when `stderr_mode == "exception_signature"`. The fuzzer runs all generated programs in this mode because generated programs may intentionally trigger exceptions via the type-safe wrapper try/except blocks

**Oracle 2 — Backend divergence (native vs WASM vs LLVM)**:

Build and run the same program on all available backends. Compare stdout across backends. Any divergence is a molt-specific bug that requires zero CPython runs to find. This oracle is the most powerful for finding optimization miscompiles because the backends share the frontend and TIR but differ in codegen. Implementation: `compile_molt_all_backends(source_path, profile, timeout, env)` calls `molt.cli build --target native`, `molt.cli build --target wasm`, and `molt.cli build --target llvm` on the same source. Compare the three stdout strings. Backends not available on the current host are skipped (macOS has native; WASM requires node.js; LLVM requires the manifest-derived `LLVM_SYS_<ver>_PREFIX` reported by `molt.llvm_toolchain`). Available-backend detection is cached at startup.

**Oracle 3 — Leak oracle (MOLT_ASSERT_NO_LEAK)**:

Set `MOLT_ASSERT_NO_LEAK=1` in the env for every molt binary execution. A non-zero exit from this flag is exit 137 (same as OOM kill from safe_run.py, distinguished by checking the stderr for `[MOLT_ASSERT_NO_LEAK] FAIL`). This oracle is free — it requires no extra builds. Any generated program that leaks is a regression.

**Oracle 4 — Compile-determinism**:

Build the same program twice with cold caches (different `MOLT_CACHE` roots, same `PYTHONHASHSEED=0`). Compare the binary hashes. This exercises the build-cache keying substrate (aaad21122). Determinism failures are miscompiles of the worst kind: the program produces different outputs on different runs. Run this oracle at 10% sampling rate on generated programs (full rate on corpus programs at promotion time).

**Oracle 5 — TIR_OPT_STATS pass-fire audit**:

Set `TIR_OPT_STATS=1` and parse the stats output. Check that no pass reports a negative fire count or an inconsistent state. This is a meta-oracle on the pass manager itself, not on program output. Also check that programs exercising the known feature families actually fire the corresponding passes: a walrus-containing program should fire SCCP; a BigInt boundary program should fire the BCE/ValueRange pass.

### 3.3 Reducer: `tools/fuzz_reducer.py` (new module, replacing the inline shrink)

**File**: `/Users/adpena/Projects/molt/tools/fuzz_reducer.py`

The existing `_shrink_program` at fuzz_compiler.py:2190 implements only top-level block removal. A complete reducer needs three strategies applied in sequence:

**Strategy 1 — Block-level delta-debug** (existing, preserve): removes top-level statements iterating to fixpoint. This is correct and fast.

**Strategy 2 — Statement-level intra-function delta-debug**: for each function/class body, remove statements one at a time. The existing strategy only removes top-level blocks but not body statements inside functions. A closure bug only reproducible inside a nested function would not be reduced below the outer function boundary by strategy 1.

**Strategy 3 — Expression simplification passes**: replace complex sub-expressions with simpler equivalents that preserve the type contract. For example: replace `f"{a + b * c}"` with `f"{42}"` if that still reproduces the failure. Implemented as AST-level rewrites using Python's `ast` module, not text manipulation:

- Replace arithmetic expressions with integer literals
- Replace f-string interpolations with string literals
- Replace function calls with their return type's default literal
- Replace comprehensions with list/dict/set literals
- Replace walrus targets with simple assignments above the enclosing expression

**Reducer contract**: a reduced program must (a) parse as valid Python, (b) reproduce the same failure category (mismatch / build_error / leak / divergence), and (c) produce the same CPython stdout as the original (so the expected output is preserved for the corpus test).

**Termination**: the reducer runs until one pass makes no progress. Maximum iterations per strategy: 50 (avoids quadratic slowdown on large programs with many removable statements).

**Integration**: the reducer runs automatically after a failure is found, before saving to `--output-dir`. The original and reduced versions are both saved. If the reduced version fails the validation check (cannot reproduce), the original is saved with a `# REDUCE_FAILED` note.

### 3.4 Corpus manager: `tools/fuzz_corpus.py` (new module)

**File**: `/Users/adpena/Projects/molt/tools/fuzz_corpus.py`

**Corpus structure**: `tests/differential/fuzzing/` (new subdirectory, parallel to `tests/differential/basic/`). Each file is a reduced failing program with:

```python
# MOLT_FUZZ_SEED: <seed>
# MOLT_FUZZ_DATE: <ISO date>
# MOLT_FUZZ_ORACLE: <oracle-name>
# MOLT_FUZZ_BUG_CLASS: <inferred-class>
# MOLT_META: xfail=molt expect_fail_reason=<class>
```

The `xfail=molt` annotation means the file is a known-failing regression until the bug is fixed, at which point the annotation is removed and the test becomes a permanent passing regression.

**Deduplication**: a failure's "fingerprint" is the SHA256 of (oracle, CPython stdout, molt stdout, first 5 differing lines). Before saving, `fuzz_corpus.py` checks the fingerprint against `tests/differential/fuzzing/known_failures.jsonl`. Identical fingerprints are discarded (the failure is already tracked). New fingerprints are appended to `known_failures.jsonl`.

**Promotion**: `fuzz_corpus.py promote <file>` moves a fuzzing-corpus file to `tests/differential/basic/` after the bug is fixed and the test passes. The xfail annotation is removed; the fuzz-origin comments are retained for provenance.

**Ratchet**: the nightly fuzzer run appends a line to `tests/differential/fuzzing/ratchet.jsonl` recording the total pass/fail counts. A CI check (`tools/check_fuzz_ratchet.py`) fails if the failure count ever increases versus the last committed ratchet line (analogous to the suite-honesty ratchet at molt_diff.py:519).

### 3.5 Extended Rust fuzz targets

**File extensions needed in `fuzz/fuzz_targets/` and `runtime/molt-backend/fuzz/fuzz_targets/`**:

**`fuzz/fuzz_targets/fuzz_tir_exception_ops.rs`** (new): extends `fuzz_tir_passes.rs` with the missing opcodes. Add `CheckException`, `TryStart`, `TryEnd`, `StateBlockStart`, `StateBlockEnd`, `IterNext`, `GetAttr`, `SetAttr`, `ModuleImport` to the `OPCODES` palette. These are the opcodes that define the exception-augmented CFG; fuzzing without them cannot find the category of bugs that caused the `needs_exception_stack` polarity trap (C2) and the `ModuleImportFrom` op routing bugs.

**`fuzz/fuzz_targets/fuzz_native_compile.rs`** (new): feeds random TirFunction structs through the native Cranelift backend codegen path. The function signature: `fn run(func: TirFunction) -> Result<Vec<u8>, CompileError>`. Must not panic. This exercises `function_compiler.rs` — the largest uninstrumented surface in the compiler.

**`fuzz/fuzz_targets/fuzz_llvm_lowering.rs`** (new, gated on `llvm` feature): feeds random TirFunction structs through `llvm/lowering.rs`. Uses the same `build_function` generator from `fuzz_tir_passes.rs` as a shared library, extracted into `fuzz/src/ir_gen.rs`.

**`fuzz/fuzz_targets/fuzz_frontend_parse.rs`** (new): feeds arbitrary byte sequences to the Python parser (rustpython-parser). Must not panic. This is the input-validation surface.

**`runtime/molt-backend/fuzz/fuzz_targets/fuzz_tir_passes.rs`** modifications: (a) expand OPCODES palette with exception-edge opcodes; (b) remove the `catch_unwind` wrapper — it is incorrect to catch panics silently; the fuzzer should let panics propagate so libFuzzer records them as crashes. The current `catch_unwind` + `resume_unwind` pattern at fuzz_tir_passes.rs:256 is correct for re-panicking, so this is not a bug, but the comment at line 262 says "Empty stats = verification failure, which is acceptable for random input" — this needs auditing because a verification failure on valid-looking input may indicate a real bug.

**New Kani harnesses**:

`runtime/molt-obj-model/tests/kani_nanbox.rs` additions:
- `bigint_boundary_not_inline_int`: proves that `MoltObject::from_int(1i64 << 46)` is NOT inline-int-tagged (it is a heap bigint). This is the 2^46 boundary that the S6/ValueRange substrate handles.
- `inline_int_range_exhaustive`: proves `from_int(i).as_int() == Some(i)` for all i in the complete 47-bit signed range via Kani's unbounded integer symbolic execution.

`runtime/molt-runtime/tests/kani_refcount.rs` additions:
- `dec_ref_ptr_immortal_skips_dealloc`: the IMMORTAL flag must cause `dec_ref_ptr` to return without calling the finalizer. This is a bounded proof of the early-return invariant.

### 3.6 CI jobs: `.github/workflows/fuzzer.yml` (new)

**File**: `/Users/adpena/Projects/molt/.github/workflows/fuzzer.yml`

Two tiers:

**PR smoke (fast, non-blocking on test failure)**:
- 100 generated programs, `--mode safe`, seed 0-99, native backend only
- Backend-divergence oracle if WASM available
- Timeout per program: 10s
- Fails CI if any `crash` or `oom` result (panic or OOM are always blocking); non-blocking on `mismatch` (these are tracked but not blocking on PR)
- Runs on: `ubuntu-latest`, triggered on push to non-main branches

**Nightly deep run (blocking)**:
- 2000 generated programs, `--mode safe` with `RiskBiasedPolicy`, seeds 0-1999
- All available oracles including leak oracle and compile-determinism at 20% sampling
- Per-program timeout: 15s
- Reduces failures automatically; saves to `tests/differential/fuzzing/`
- Runs the suite-honesty ratchet check on the fuzzing corpus
- Triggered: weekly Saturday 2am UTC (offset from the Monday Nightly and Tuesday Sanitizer jobs)
- Uses `matrix.strategy.fail-fast: false` so all 2000 programs run even if early failures occur

**SIGURG mitigation in both tiers**: fuzz programs run in batches of 40 per subprocess call. Each batch spawns a fresh Python interpreter with a fresh `MOLT_SESSION_ID` so no daemon connection persists across batches. The batch loop re-execs itself every 40 programs. This is the "batch+re-exec" pattern from the wave-1 baton.

---

## Part 4 — Implementation Map

### Files to create

| File | Purpose | Size estimate |
|---|---|---|
| `/Users/adpena/Projects/molt/tools/fuzz_generator.py` | Extracted + extended `SafeProgramGenerator` with 10 new feature families and `FeaturePolicy` | ~700 lines |
| `/Users/adpena/Projects/molt/tools/fuzz_oracles.py` | Five oracle implementations; `OracleResult` dataclass; `run_oracle_stack()` | ~300 lines |
| `/Users/adpena/Projects/molt/tools/fuzz_reducer.py` | AST-aware reducer with 3 strategies | ~400 lines |
| `/Users/adpena/Projects/molt/tools/fuzz_corpus.py` | Corpus management, dedup, ratchet, promotion | ~250 lines |
| `/Users/adpena/Projects/molt/fuzz/fuzz_targets/fuzz_tir_exception_ops.rs` | TIR fuzzer extended with exception-edge opcodes | ~150 lines |
| `/Users/adpena/Projects/molt/fuzz/fuzz_targets/fuzz_native_compile.rs` | Cranelift backend no-panic fuzzer | ~200 lines |
| `/Users/adpena/Projects/molt/fuzz/fuzz_targets/fuzz_frontend_parse.rs` | Parser no-panic fuzzer | ~40 lines |
| `/Users/adpena/Projects/molt/fuzz/src/ir_gen.rs` | Shared TirFunction generator extracted from fuzz_tir_passes | ~200 lines |
| `/Users/adpena/Projects/molt/.github/workflows/fuzzer.yml` | PR smoke + nightly deep CI jobs | ~120 lines |
| `/Users/adpena/Projects/molt/tests/differential/fuzzing/known_failures.jsonl` | Empty seed file (touch) | 0 lines |
| `/Users/adpena/Projects/molt/tests/differential/fuzzing/ratchet.jsonl` | Empty seed file (touch) | 0 lines |

### Files to modify

| File | Change | Why |
|---|---|---|
| `/Users/adpena/Projects/molt/tools/fuzz_compiler.py` | Replace `SafeProgramGenerator` body with `from tools.fuzz_generator import SafeProgramGenerator`; replace `_shrink_program` with `from tools.fuzz_reducer import shrink_program`; add `--backend-divergence` flag wiring to oracle stack; add `--mode backend-diff` entry point | Preserves existing CLI and existing runners, replaces only the generator and reducer internals |
| `/Users/adpena/Projects/molt/fuzz/Cargo.toml` | Add `fuzz_tir_exception_ops`, `fuzz_native_compile`, `fuzz_frontend_parse` binary entries; add `molt-backend` with `native-backend` feature to dependencies | Wire new targets |
| `/Users/adpena/Projects/molt/fuzz/src/lib.rs` (new) | `pub mod ir_gen;` — shared TirFunction generator | Shared code extraction |
| `/Users/adpena/Projects/molt/runtime/molt-backend/fuzz/fuzz_targets/fuzz_tir_passes.rs` | Expand `OPCODES` palette with exception-edge opcodes; add a comment on the catch_unwind invariant | Close the exception-CFG coverage gap |
| `/Users/adpena/Projects/molt/runtime/molt-obj-model/tests/kani_nanbox.rs` | Add `bigint_boundary_not_inline_int` and `inline_int_range_exhaustive` harnesses | Close 2^46 boundary gap in formal verification |
| `/Users/adpena/Projects/molt/runtime/molt-runtime/tests/kani_refcount.rs` | Add `dec_ref_ptr_immortal_skips_dealloc` harness | Close immortal-dealloc invariant gap |
| `/Users/adpena/Projects/molt/runtime/molt-obj-model/KANI.md` | Update "What is NOT yet verified" section with new harnesses; update total count | Documentation accuracy |
| `/Users/adpena/Projects/molt/.github/workflows/nightly.yml` | Add reference to `fuzzer.yml` ratchet check in the nightly job matrix | Connect ratchet to nightly gate |
| `/Users/adpena/Projects/molt/tools/check_fuzz_ratchet.py` (new) | Read `tests/differential/fuzzing/ratchet.jsonl`; assert current failure count <= last committed count | Ratchet enforcement |

---

## Part 5 — Data Flow

**Generation flow**:

```
Random(seed) → FeaturePolicy → SafeProgramGenerator.generate() → source.py
```

**Execution and oracle flow**:

```
source.py
  ├── CPython(source.py, PYTHONHASHSEED=0)  → (stdout_c, stderr_c, rc_c)
  ├── molt build --target native             → binary_n
  │     └── molt run binary_n               → (stdout_n, stderr_n, rc_n)
  ├── molt build --target wasm              → binary_w (if available)
  │     └── node binary_w                  → (stdout_w, stderr_w, rc_w)
  ├── molt build --target llvm             → binary_l (if available)
  │     └── ./binary_l                     → (stdout_l, stderr_l, rc_l)
  └── MOLT_ASSERT_NO_LEAK=1 molt run binary_n → check exit 137 + stderr marker

Oracle 1: stdout_n == stdout_c AND exit_code_compatible(rc_n, rc_c)
Oracle 2: stdout_n == stdout_w == stdout_l (where available)
Oracle 3: no [MOLT_ASSERT_NO_LEAK] FAIL in stderr
Oracle 4 (10% sample): hash(binary_n_build1) == hash(binary_n_build2)
Oracle 5: TIR_OPT_STATS pass counts are self-consistent
```

**Failure handling flow**:

```
OracleResult(fail, oracle_id, seed, source)
  → fuzz_reducer.shrink_program(source, oracle) → reduced_source
  → fuzz_corpus.fingerprint(oracle_id, stdout_c, stdout_n)
  → deduplicate against known_failures.jsonl
  → if NEW: save tests/differential/fuzzing/fuzz_<date>_<seed>.py
            append known_failures.jsonl
            append ratchet.jsonl
  → print triage classification
```

**Coverage feedback loop**:

```
After batch of 40 programs:
  Parse TIR_OPT_STATS from stderr of each build
  Update PassFireCounts[pass_name] += count
  Next batch: weight seeds that fired infrequent passes 2× higher
```

---

## Part 6 — Build Sequence (Phased Checklist)

### Phase 1 — Generator extension and oracle stack (acceptance gate: must find or prove-absent at least one new real bug class from the known list)

- [ ] Create `tools/fuzz_generator.py` with the 10 new feature families added to `SafeProgramGenerator`
- [ ] Validate the generator: run 500 programs through CPython alone (no molt build) and verify <5% CPython-error rate. If higher, the new feature families are generating invalid programs and must be fixed before continuing.
- [ ] Add walrus generator and verify: `[x for x in [1,2,3] if (y := x*2) > 3]` must produce identical CPython output across all seeds
- [ ] Add f-string `{expr=}` generator
- [ ] Add BigInt boundary literal generator: `INT_RANGE` pool extended with `[-(2**63), -(2**60), -(2**46), -1, 0, 1, 2**46-1, 2**46, 2**60, 2**63-1]` as weighted candidates
- [ ] Add nonlocal augmented-assign closure shape
- [ ] Add generator shape family
- [ ] Add match/case generator
- [ ] Add cross-scope `in` membership on dict/set
- [ ] Add module-scope from-import generator
- [ ] Create `tools/fuzz_oracles.py` with Oracle 1 (CPython differential, using `_extract_exception_signature`) and Oracle 3 (MOLT_ASSERT_NO_LEAK)
- [ ] Wire oracle stack into `fuzz_compiler.py` `fuzz_one_safe` function
- [ ] **Calibration test**: run 200 programs with seeds 0-199 through CPython + native oracle. If zero mismatches are found, run the seeds 200-499 (the generator must find a new bug in the known list OR demonstrate zero false positives on 500 clean programs to establish oracle precision).

**Phase 1 acceptance gate**: the calibration test either finds a genuine mismatch on a known feature family (walrus/f-string-debug/BigInt/nonlocal), OR logs zero false positives on 500 programs demonstrating oracle precision is sufficient to proceed to CI.

### Phase 2 — Backend-divergence oracle and SIGURG-immune executor

- [ ] Create `tools/fuzz_oracles.py` Oracle 2 (backend divergence)
- [ ] Add available-backend detection at startup
- [ ] Implement batch executor with SIGURG mitigation: batch size 40, re-exec between batches, JSONL append for results
- [ ] Add `--mode backend-diff` to fuzz_compiler.py: runs only the backend-divergence oracle without CPython, enabling fast iteration without CPython startup cost
- [ ] Test: run 100 backend-diff programs on native vs WASM and verify no false positives from non-determinism (PYTHONHASHSEED=0 and MOLT_DETERMINISTIC=1 must be set on all backends)

### Phase 3 — Reducer and corpus infrastructure

- [ ] Create `tools/fuzz_reducer.py` with all three strategies
- [ ] Validate reducer: take the 5 known corpus files in `tests/differential/basic/` that contain walrus/f-string patterns and verify the reducer produces programs ≤30% of original size while preserving the mismatch
- [ ] Create `tools/fuzz_corpus.py` with fingerprinting, dedup, ratchet
- [ ] Create `tests/differential/fuzzing/` directory with seed JSONL files
- [ ] Wire reducer and corpus manager into the fuzzer main loop
- [ ] Create `tools/check_fuzz_ratchet.py`

### Phase 4 — Rust fuzz target hardening

- [ ] Expand `runtime/molt-backend/fuzz/fuzz_targets/fuzz_tir_passes.rs` OPCODES palette
- [ ] Create `fuzz/src/ir_gen.rs` shared TirFunction generator
- [ ] Create `fuzz/fuzz_targets/fuzz_tir_exception_ops.rs`
- [ ] Create `fuzz/fuzz_targets/fuzz_native_compile.rs`
- [ ] Create `fuzz/fuzz_targets/fuzz_frontend_parse.rs`
- [ ] Update `fuzz/Cargo.toml` with new binary entries and native-backend feature dependency
- [ ] Verify all new targets compile: `cargo fuzz build --fuzz-dir fuzz fuzz_tir_exception_ops` (requires nightly)
- [ ] Add new Kani harnesses to `kani_nanbox.rs` and `kani_refcount.rs`
- [ ] Update `KANI.md` total count and "not yet verified" section

### Phase 5 — CI integration and coverage feedback loop

- [ ] Create `.github/workflows/fuzzer.yml` with PR smoke and nightly jobs
- [ ] Add SIGURG-immune batch parameters to CI env
- [ ] Wire `TIR_OPT_STATS=1` and implement `PassFireCounts` tracking in the batch executor
- [ ] Implement the `RiskBiasedPolicy` for nightly runs
- [ ] Add ratchet check to `nightly.yml`
- [ ] Full nightly dry-run locally with 500 programs, verify JSONL output and ratchet file are written correctly
- [ ] Commit seed `known_failures.jsonl` and `ratchet.jsonl` files

---

## Part 7 — Critical Details

### Error handling

**OOM in fuzz binary**: a generated program with an unbounded loop (e.g., generated `while True:` with no bound increment — the while generator at fuzz_compiler.py:876 always generates a bounded counter, but the new match/case and generator shapes could produce iteration patterns the bound checker misses) must not bring down the host. The `harness_memory_guard.guarded_completed_process` call on binary execution already provides the RSS cap. The leak oracle's `MOLT_ASSERT_NO_LEAK=1` provides an additional signal that distinguishes a true memory leak (grows unboundedly per iteration) from a program that merely uses a lot of memory (fixed high watermark).

**Build-daemon cross-contamination**: the fuzzer must set a unique `MOLT_SESSION_ID` per batch (e.g., `fuzz-batch-<pid>-<batch_n>`) to route to its own daemon socket. Without this, a fuzz build that panics the daemon will kill other sessions' daemons. Reference: CLAUDE.md Concurrent Development section.

**Version mismatch in generated programs**: generated programs that contain `match/case` are 3.10+ syntax; programs with `except*` are 3.11+ syntax. The generator must gate these on the detected CPython version via `_python_exe_version()` at molt_diff.py:231. If the baseline CPython is 3.12, all features through 3.12 are available. A `MinVersionGate` in `FeaturePolicy` enforces this.

**Nondeterminism suppression**: all dict/set output in generated programs must be printed via `sorted()`. The generator already enforces this (dict-iteration at fuzz_compiler.py:933 uses `sorted(d.items())`). The new feature families must enforce the same constraint. Any use of `hash()` on user-defined objects is forbidden (unless wrapped in `PYTHONHASHSEED=0`-safe patterns). The `id()` builtin is forbidden in print statements.

### State management

**Session isolation**: each batch uses `MOLT_SESSION_ID=fuzz-<uuid>` so cargo builds and daemon sockets are isolated per batch. The session directory grows over a long run; add a cleanup call after each 200-program epoch: `molt clean --apply --kill-processes` with the fuzz session ID filter.

**JSONL append safety**: the JSONL files (`known_failures.jsonl`, `ratchet.jsonl`, `MOLT_DIFF_RESULTS_JSONL`) are written with `open(path, "a")` which is atomic for line-sized appends on POSIX. No additional locking is needed because the fuzz loop is single-threaded per batch (concurrent batches would require fcntl locking as in molt_diff.py:648).

### Testing the fuzzer itself

**Self-calibration**: the Phase 1 calibration test (200 programs through CPython only) verifies generator validity. A generator that produces >5% CPython-error programs has a semantic bug in the new feature families.

**Regression test for the reducer**: use the 5 known walrus/f-string corpus files as inputs to the reducer and verify the reduced output is ≤ a known bound (e.g., 15 lines for a walrus bug). This test runs as a unit test in `tools/tests/test_fuzz_reducer.py`.

**Known-bug calibration**: for each of the 5 recently found bug classes (walrus, f-string `{expr=}`, BigInt boundaries, nonlocal augmented-assign, cross-scope membership), verify that the generator with the new feature families produces programs that trigger the bug when run against a BUILD that predates the fix. This is the acceptance gate for Phase 1: if the generator cannot reproduce the known bugs, it will not find new ones.

### Performance

**Build cost**: the dominant cost is the molt build per program (5s on a warm daemon, ~30s cold). The fuzzer must use the batch compile server when available (`_diff_batch_compile_server_enabled()` at molt_diff.py:1211). Batch size of 40 programs per daemon call amortizes startup cost.

**CPython cost**: CPython startup is ~18ms vs molt's ~5ms. Running CPython on 2000 programs adds ~36s. This is acceptable for nightly; for the PR smoke run the CPython oracle is run only on programs that *pass* the backend-divergence oracle, under the assumption that backend divergence is the higher-yield oracle for new bugs.

**Parallelism budget**: the CLAUDE.md policy caps concurrent builds at 2. The fuzzer uses a simple sequential loop within a batch (no parallelism within the batch). Two concurrent fuzzer invocations (e.g., two CI runners) would violate this cap. The fuzzer writes a `fuzz_run.lock` file analogous to `_diff_run_lock_path()` at molt_diff.py:609 and acquires it with `fcntl.LOCK_EX` before starting builds.

### Security

Generated programs are never executed with elevated privileges. The `harness_memory_guard` isolates execution. Generated programs never have access to network (the fuzzer sets `MOLT_DISABLE_NETWORKING=1` if that env var is supported by the runtime, otherwise relies on the OS-level network isolation of the test environment).

---

## Part 8 — Risk Register

**Risk 1 — Flaky oracle noise from PYTHONHASHSEED non-coverage**

Mitigation: PYTHONHASHSEED=0 and MOLT_DETERMINISTIC=1 are set globally. The `sorted()` enforcement in the generator eliminates hash-order sensitivity. Any program that produces non-deterministic output with fixed PYTHONHASHSEED has a genuine non-determinism bug in molt.

**Risk 2 — CPython version drift in generated programs**

Mitigation: the `MinVersionGate` in `FeaturePolicy` gates syntax features on the detected baseline version. The `_molt_sys_env_for_python_exe` call at run time ensures molt's version-gated messages match the baseline. Any remaining drift is a genuine parity bug, not a false positive.

**Risk 3 — Generation bias blind spots**

The `FeaturePolicy` approach (following YARPGen's generation-policies insight) biases toward known risk families. But unknown risk families — feature interactions not in the known list — receive no bias. Mitigation: run a parallel `--mode compile-only` with hypothesmith on 200 programs per nightly run to maintain grammar-level coverage of the full Python surface, even if the differential signal is limited. Any crash in compile-only mode is immediately actionable.

**Risk 4 — Rust fuzz target maintenance**

The TIR opcode set grows with each new arc (e.g., `ModuleImportFrom` was added this week). Every new opcode must be added to the OPCODES palette in the fuzz targets. Mitigation: add a Rust unit test in `fuzz_tir_passes.rs` that asserts the OPCODES palette covers all opcodes in the `OpCode` enum. This test will fail at compile time whenever a new opcode is added without updating the palette.

**Risk 5 — SIGURG kills on macOS CI**

Mitigation: batch size 40 + re-exec architecture. The 15-minute SIGURG timeout on this host means batches under ~10 minutes are safe. At 5s/program, 40 programs = ~200s = well under 10 minutes.

**Risk 6 — Backend-divergence oracle false positives from WASM float semantics**

WASM has slightly different float rounding for some operations (`f64.div`, `f64.nearest`). Mitigation: the generator's `_gen_float_expr` avoids division and rounds intermediate results to 4 decimal places (`gen_float_literal` returns `f"{val:.4f}"`). The backend-divergence oracle should add a float-tolerance comparison mode (compare up to 6 significant digits) for float-containing programs as a secondary comparison, falling back to byte-exact for integer/string programs.

**Risk 7 — Reducer introduces new false passing**

A reduced program that changes semantics (e.g., removes a print that produced the diverging output) would falsely appear to pass after reduction. Mitigation: the reducer validates that the original failure's CPython stdout is preserved in the reduced version. If CPython output changes during reduction, that candidate is rejected.

---

## References and Citations

- YARPGen: Livinskii, Y., Babokin, D., & Regehr, J. (2020). Random testing for C and C++ compilers with YARPGen. *OOPSLA 2020*. [doi:10.1145/3428264](https://dl.acm.org/doi/10.1145/3428264) — generation-policies mechanism for biasing toward optimization-triggering patterns.
- CSmith: Yang, X., Chen, Y., Eide, E., & Regehr, J. (2011). Finding and understanding bugs in C compilers. *PLDI 2011*. [Utah preprint](https://users.cs.utah.edu/~regehr/papers/pldi12-preprint.pdf) — type-tracked generation to avoid undefined behavior; probability table over usable variables at each program point.
- hypothesmith: Zac-HD. [github.com/Zac-HD/hypothesmith](https://github.com/Zac-HD/hypothesmith) — grammar-based Python generation; found CPython BPO-40661 (segfault in parser that blocked 3.9 beta1).
- fusil / lafleur: devdanzin. [github.com/devdanzin/fusil](https://github.com/devdanzin/fusil) / [github.com/devdanzin/lafleur](https://github.com/devdanzin/lafleur) — 52 CPython issues in 6 months; coverage-guided CPython fuzzer.
- Jit-Picking: Bernhard, L. et al. (2022). Differential fuzzing of JavaScript engines. *CCS 2022*. [dl.acm.org](https://dl.acm.org/doi/pdf/10.1145/3548606.3560624) — patching engines to suppress non-deterministic builtins for clean differential oracle.
- Ratte: ASPLOS 2025. [doc.ic.ac.uk paper](https://www.doc.ic.ac.uk/~afd/papers/2025/ASPLOS-Ratte.pdf) — differential testing of MLIR optimization passes; found 8 bugs including 6 miscompilations — the exact analog for molt's TIR pass pipeline.
- CsmithEdge: Springer Nature Link. [link.springer.com](https://link.springer.com/article/10.1007/s10664-022-10146-1) — less conservative UB handling for broader compiler trigger coverage.
- Hypothesis shrinking: HypothesisWorks. [strategies-that-shrink.rst](https://github.com/HypothesisWorks/hypothesis/blob/master/guides/strategies-that-shrink.rst) — integrated shrinking via choice-sequence minimization, preserving generator validity constraints during reduction.
- cargo-fuzz / arbitrary: rust-fuzz. [rust-fuzz.github.io/book](https://rust-fuzz.github.io/book/cargo-fuzz.html) — structured fuzzing with `#[derive(Arbitrary)]`; coverage-guided via libFuzzer.
- Kani: model-checking. [model-checking.github.io/kani](https://model-checking.github.io/kani/usage.html) — bounded model checking for Rust; `#[cfg(kani)]` gates harnesses; `BoundedArbitrary` for bounded proofs.
- Survey of Modern Compiler Fuzzing: arxiv.org/pdf/2306.06884 — differential testing vs metamorphic testing oracle taxonomy; multi-backend comparison design patterns.

Sources:
- [Random Testing for C and C++ Compilers with YARPGen](https://dl.acm.org/doi/pdf/10.1145/3428264)
- [GitHub - Zac-HD/hypothesmith](https://github.com/Zac-HD/hypothesmith)
- [GitHub - devdanzin/fusil](https://github.com/devdanzin/fusil)
- [GitHub - devdanzin/lafleur](https://github.com/devdanzin/lafleur)
- [Jit-Picking: Differential Fuzzing of JavaScript Engines](https://dl.acm.org/doi/pdf/10.1145/3548606.3560624)
- [Ratte: Fuzzing for Miscompilations in Multi-Level Compilers](https://www.doc.ic.ac.uk/~afd/papers/2025/ASPLOS-Ratte.pdf)
- [CsmithEdge: more effective compiler testing](https://link.springer.com/article/10.1007/s10664-022-10146-1)
- [Hypothesis strategies-that-shrink guide](https://github.com/HypothesisWorks/hypothesis/blob/master/guides/strategies-that-shrink.rst)
- [Structure-Aware Fuzzing - Rust Fuzz Book](https://rust-fuzz.github.io/book/cargo-fuzz/structure-aware-fuzzing.html)
- [Using Kani - The Kani Rust Verifier](https://model-checking.github.io/kani/usage.html)
- [A Survey of Modern Compiler Fuzzing](https://arxiv.org/pdf/2306.06884)
- [Fuzzing Loop Optimizations in Compilers (YARPGen 2023)](https://users.cs.utah.edu/~regehr/pldi23.pdf)
