<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: DX / shortest-wall-clock: decompose the molt-runtime/molt-backend monoliths for fast incremental builds + parallel agent throughput -->

# Build-Time / DX Architecture Blueprint

## 1. Precise Problem Statement

**Why this is load-bearing for the 5-year program:** Every compiler-foundation arc (E1 inliner activation, S5 MemSSA, E5 monomorphization, Tier-3 CoroElide) requires landing a multi-thousand-line structural change with 5+ agents working in parallel. The current bottleneck is not design quality — it is that every edit to any file in `molt-runtime` (344K lines, verified by the design doc) or `function_compiler.rs` (38,510 lines, measured) re-runs the entire compile-link cycle before a single test can execute. At 5-minute cold builds and ~2-3 minute incremental builds for these modules, three agents each triggering a rebuild after landing their piece costs 6-9 minutes of wall-clock per iteration cycle. A 10-phase arc that requires 15 cycles is 90-135 minutes of pure wait. That is the compound DX tax that makes the 5-year mission slip.

**Three measured bottlenecks:**

1. `release-fast` uses `lto = "fat"` (verified: `/Users/adpena/Projects/molt/Cargo.toml:295`). Fat LTO merges all bitcode into a single module and re-optimizes in one serial phase — this collapses the parallelism that `codegen-units = 256` (line 306) would otherwise give. The comment on lines 296-305 is honest: "the cgu count has negligible effect on the final binary's runtime perf" under fat LTO. For the daemon iteration loop there is no perf justification for fat LTO.

2. `function_compiler.rs` is 38,510 lines in a single file compiled as one compilation unit. Every edit to any NaN-box helper, every new opcode handler, every new intrinsic call site requires recompiling the entire file. With `codegen-units = 256` this still means one of those 256 units is 38K lines.

3. `sccache` is opt-in via `_maybe_enable_sccache` (cli.py:9361) with `MOLT_USE_SCCACHE=auto` defaulting to "auto" but silently skipping if `sccache` is not found (cli.py:9367-9370). Multi-agent worktrees today each compile from the same source crates but share no compiled-rlib cache between sessions. The agent worktrees under `.claude/worktrees/` each start from a session-start commit, meaning every agent 1-builds from scratch.

**Secondary DX bottlenecks:**
- `TIR_DUMP=1` is a blunt per-process toggle with no per-function filter or per-pass gate (printer.rs:18-20). Pass-by-pass TIR snapshots require MOLT_DUMP_IR (lib.rs:97) + TIR_DUMP separately, with no unified "dump TIR after pass X for function F" ergonomic.
- `MOLT_VERIFY_ANALYSIS=1` (pass_manager.rs:173) is a post-pass full-recompute check but has no per-pass skip list or pass-name filter, making it expensive when narrowing down which pass introduced a stale analysis.
- `tests/molt_diff.py` is the differential regression harness but requires `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` to be set correctly (cli.py:30058-30100); misconfiguration silently runs against a stale artifact.

## 2. The Structurally-Correct Design

### Decision: Three-phase arc with clear structural boundaries

**Phase 1 (config, immediate, no source changes):** Switch `release-fast` to `lto = "thin"` for the backend-daemon build product. Add a new `release-output` profile for shipped user-binary artifacts that retains `lto = "fat"`. Flip sccache to default-on with a shared sccache directory across worktrees. Activate `ld64.lld` (macOS) / `mold` (Linux) for dev profiles.

**Phase 2 (structural, highest leverage):** Split `function_compiler.rs` into 8-10 logically-cohesive sub-modules within the existing `native_backend/` tree. Each sub-module maps to one opcode family (arithmetic, control-flow, collections, exceptions, closures, trampolines, loops, async/generators). The `mod.rs` shims exports identically. This is a within-crate module split — no cross-crate boundary change, no feature-flag complications, no API surface change. Rebuild blast radius drops from "38K lines" to "the changed opcode family module only."

**Phase 3 (structural, highest overall impact):** Extract `molt-backend-native` as its own workspace crate. This is the scope documented in the design doc (lines 119-138 of `parallel_build_architecture.md`): `native_backend/*` + `llvm_backend/` become `molt-backend-native`, depending on `molt-backend` (core: tir/passes/ir/representation_plan). Editing Cranelift codegen no longer triggers recompile of TIR passes. Editing TIR passes no longer triggers recompile of Cranelift codegen (in the reverse direction). Both compilations run in parallel.

**Deferred (addressed by the existing decomposition):** `molt-runtime` is already substantially decomposed — the workspace shows 18 extracted leaf crates (`molt-runtime-crypto`, `-net`, `-asyncio`, `-math`, `-path`, `-collections`, `-regex`, `-text`, `-itertools`, `-serial`, `-difflib`, `-logging`, `-http`, `-stringprep`, `-xml`, `-ipaddress`, `-zoneinfo`, `-compression`). The residual `molt-runtime` monolith (still ~344K lines per the design doc) retains the core object model, `builtins/` directory, `object/` directory, and the intrinsic registry. The generated intrinsic resolver source split has landed: `intrinsics/generated.rs` retains the parser-facing `INTRINSICS` manifest table and delegates to per-category modules under `intrinsics/generated_resolvers/`. Native still uses the per-app resolver to keep the whole-registry static resolver unreachable in shipped binaries; test/WASM builds retain the composed resolver path.

**The intrinsic registry split:** the source-file split is complete for resolver bodies. The remaining structural arc is per-crate sub-registries: as runtime leaves take ownership of their intrinsic implementations, their generated resolver modules should move with the leaf crate and the `molt-runtime` facade should compose them through one thin resolver.

### Why thin LTO is correct for the daemon

The backend daemon compiles Python to native/WASM/LLVM bitcode. Its hot paths are: TIR pass pipeline (intra-crate, not cross-crate), Cranelift IR construction (intra-crate), and JSON deserialization of the SimpleIR transport. None of these benefit measurably from cross-crate whole-program optimization. The `lto = "fat"` in `release-fast` pays a serial LTO phase of 30-60 seconds on every rebuild of any backend crate to deliver at best a few percent improvement in Cranelift's own internal hot paths — paths that are already compiler-optimized by Cranelift's own release profile. Thin LTO gives parallel codegen + inter-crate inlining at the import boundary (the only cross-crate calls that matter for the daemon) in a parallel multi-threaded phase. The shipped user-binary runtime (`release-output`, a distinct product) retains fat LTO because that binary's cross-crate calls (user code calling runtime intrinsics) ARE the hot path.

### DX improvements (Phase 1 additions)

**`MOLT_TIR_DUMP`:** Extend `tir_dump_enabled()` (printer.rs:18) to accept `MOLT_TIR_DUMP=fn_name:pass_name` where `fn_name` and `pass_name` are optional filters. The pass_manager.rs already checks `MOLT_DUMP_IR` and `TIR_DUMP` at line 189; unify them under one env var with filter syntax.

**`MOLT_VERIFY_ANALYSIS`:** Extend from a boolean to accept a pass-name filter: `MOLT_VERIFY_ANALYSIS=gvn` re-verifies only after the GVN pass, not after every pass. The current check at pass_manager.rs:173 becomes `verify_analysis_for_pass(pass_name)`.

**`molt_diff` target validation:** Add a `molt dx check` subcommand (or extend the existing `molt clean --apply` path) that verifies `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` are set, consistent, and pointing to a non-stale artifact before any differential test run. The current validation at cli.py:30058-30100 exists but requires the user to know to call it.

## 3. Component Design

### Phase 1 Components

**A. `/Users/adpena/Projects/molt/Cargo.toml`**
- Modify `[profile.release-fast]` (line 292): change `lto = "fat"` to `lto = "thin"`. Keep `codegen-units = 256`.
- Add new `[profile.release-output]` inheriting from `release` with `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `panic = "unwind"`. This is the profile for shipping the daemon binary to end-users, not for developer iteration.
- Add `[profile.release-fast.package.molt-backend-native]` (post Phase 3) with `opt-level = 3, codegen-units = 256` matching the `molt-backend` override pattern already established at lines 53-55.

**B. `/Users/adpena/Projects/molt/.cargo/config.toml`**
- Add macOS fast-linker configuration under `[target.aarch64-apple-darwin]`: `rustflags = ["-C", "link-arg=-fuse-ld=/usr/local/bin/ld64.lld"]` (conditional on the binary existing, or as a separate config-fast-link-macos.toml following the config-fast-link.toml pattern).
- Clarify that `config-fast-link.toml` (currently Linux-only) is the opt-in mechanism; document the macOS equivalent.

**C. `/Users/adpena/Projects/molt/src/molt/cli.py`**
- `_maybe_enable_sccache` (line 9361): change the default from implicit `auto` to explicit `on` when `sccache` is found. The current logic already does this when `MOLT_USE_SCCACHE` is unset and sccache is in PATH. The change is: set `SCCACHE_DIR` to a project-root-relative shared cache directory (`$project_root/.sccache/`) rather than the sccache default (`~/.cache/sccache`). This makes the cache shared across all agent worktrees (they all have the same `project_root`) while keeping it repo-local for easy cleanup.
- Add `SCCACHE_CACHE_SIZE` default (e.g. `"20G"`) to prevent unbounded cache growth.
- `_session_target_dir` (line 12562): already correct. No change needed.

### Phase 2 Components: `function_compiler.rs` module split

The 38,510-line file is split into sub-modules within `runtime/molt-backend/src/native_backend/`. The split is by opcode family as found in the existing code structure:

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_arith.rs`**
- Responsibility: all arithmetic ops — `IAdd`, `ISub`, `IMul`, `IDiv`, `IMod`, `IPow`, `FAdd`, `FSub`, `FMul`, `FDiv`, `IShift`, `IBitwise`, `IUnary`, `FUnary`, `IComp`, `FComp`, and the NaN-box mixed-type arithmetic bridge.
- Estimated size: ~6,000 lines.
- Dependencies: `NanBoxConsts`, `repr::Repr`, `VarValue`, `block_has_terminator`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_control.rs`**
- Responsibility: control flow — `If`/`Else`/`EndIf`, `Jump`, `BrIf`, `LoopStart`/`LoopEnd`, `LoopBreakIfTrue`/`LoopBreakIfException`/`LoopContinue`, `Ret`, `StateTransition`/`StateYield`.
- Estimated size: ~4,000 lines.
- Dependencies: `switch_to_block_tracking`, `extend_unique_tracked`, `DeferredDefine`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_collections.rs`**
- Responsibility: list/dict/set/tuple ops — `ListAppend`, `ListPop`, `ListIndex`, `ListStoreIndex`, `DictGet`, `DictSet`, `TupleIndex`, `SetAdd`, `Contains`, `Unpack`.
- Estimated size: ~5,000 lines.
- Dependencies: `vec_layout::vec_u64_layout`, `repr::ContainerKind`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_exceptions.rs`**
- Responsibility: exception handling — `TryStart`/`TryEnd`, `CheckException`, `Raise`, `ExceptionPending`, `LoopBreakIfException`, exception-stack push/pop intrinsic calls.
- Estimated size: ~3,000 lines.
- Dependencies: `pending_bits()`, intrinsic symbol constants.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_closures.rs`**
- Responsibility: closure/function ops — `MakeFunction`, `MakeClosure`, `LoadAttr`, `StoreAttr`, `Call`, `CallMethod`, `IncRef`/`DecRef`, `RC` coalescing emission.
- Estimated size: ~5,000 lines.
- Dependencies: `TrampolineSpec`, `TrampolineKind`, `stable_ic_site_id`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_trampolines.rs`**
- Responsibility: trampoline emission — `emit_trampoline`, `emit_generator_trampoline`, `emit_coroutine_trampoline`, `emit_async_gen_trampoline`, `TrampolineKey` table management. Already partially in simple_backend.rs around the TrampolineKey definition.
- Estimated size: ~3,000 lines.
- Dependencies: `TrampolineKey`, `TrampolineKind`, `TrampolineSpec`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_loops.rs`**
- Responsibility: loop-specific codegen — `LoopIndexStart`, list/array data-pointer hoisting (`scan_loop_hoistable_lists` is currently at function_compiler.rs:48), loop-IV emission, loop-index pre/post increment.
- Estimated size: ~3,000 lines.
- Dependencies: `scan_loop_hoistable_lists` (moves here from function_compiler.rs).

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/fc_async.rs`**
- Responsibility: async/generator state-machine codegen — `StateLabelStart`/`StateLabelEnd`, `StateBlockStart`/`StateBlockEnd`, `ChanSendYield`/`ChanRecvYield`, coroutine frame ops.
- Estimated size: ~4,000 lines.
- Dependencies: `TrampolineSpec`, `function_requires_value_return`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs`** (residual)
- After the split: ~5,510 lines containing the `FunctionCompiler` struct, its `new()`, the top-level `compile_function()` dispatch, the `emit_op()` match arm dispatcher, and any cross-cutting helpers that do not fit a single family.
- The sub-modules are declared `pub(super) mod fc_arith; ...` and their entry points are called from `emit_op()`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/mod.rs`** (unchanged structurally)
- Adds `mod fc_arith; mod fc_control; ...` declarations (or declares them in `function_compiler.rs` via `use super::fc_arith`). Since the sub-modules are siblings, the cleanest declaration point is `native_backend/mod.rs` alongside the existing `mod function_compiler;`.

### Phase 3 Components: `molt-backend-native` crate extraction

**New file: `/Users/adpena/Projects/molt/runtime/molt-backend-native/Cargo.toml`**
```toml
[package]
name = "molt-backend-native"
version = "0.1.0"
publish = false
edition = "2024"
license = "Apache-2.0"

[dependencies]
molt-backend = { path = "../molt-backend", default-features = false }
cranelift-codegen = { version = "0.131.0", optional = true, ... }
cranelift-frontend = { version = "0.131.0", optional = true }
cranelift-module = { version = "0.131.0", optional = true }
cranelift-object = { version = "0.131.0", optional = true }
cranelift-native = { version = "0.131.0", optional = true }
inkwell = { version = "0.8", features = ["llvm21-1"], optional = true }

[features]
default = ["native-backend"]
native-backend = ["dep:cranelift-codegen", ...]
llvm = ["dep:inkwell"]
```

**Move:** `runtime/molt-backend/src/native_backend/` directory becomes `runtime/molt-backend-native/src/native_backend/`. `runtime/molt-backend/src/llvm_backend/` becomes `runtime/molt-backend-native/src/llvm_backend/`. `runtime/molt-backend/src/native_backend_consts.rs` moves to `runtime/molt-backend-native/src/`.

**Preserve in `molt-backend` (core):**
- `tir/` (all of it: passes, analysis, pass_manager, module_phase, parallel, etc.)
- `ir.rs`, `ir_rewrites.rs`, `ir_schema.rs`, `json_boundary.rs`
- `representation_plan.rs`
- `intrinsic_symbols.rs`
- `debug_artifacts.rs`
- `passes.rs` (SimpleIR passes — these do NOT depend on Cranelift)
- `wasm.rs`, `wasm_imports.rs`, `tir/lower_to_wasm.rs` (wasm stays in molt-backend since it has no Cranelift dep)
- `luau_ir.rs`, `luau_lower.rs`, `luau.rs`, `rust.rs` (transpilers)
- `egraph_simplify.rs`

**`molt-backend` lib.rs changes:** Remove `#[cfg(feature = "native-backend")] mod native_backend;` and the `pub use native_backend::*` re-exports. The `SimpleBackend`, `CompileOutput`, `NativeBackendModuleContext` types are re-exported by `molt-backend-native` directly. The daemon's `main.rs` adds `extern crate molt_backend_native;` and adjusts imports.

**`/Users/adpena/Projects/molt/Cargo.toml` workspace members:** Add `"runtime/molt-backend-native"`.

**Build command changes:** The CLAUDE.md instruction `cargo build --profile release-fast -p molt-backend --features native-backend` becomes `cargo build --profile release-fast -p molt-backend-native --features native-backend`. Add an alias. The backend daemon binary (`main.rs`) lives in `molt-backend-native` as its `[[bin]]`, since it is the entry point that stitches together core + native.

### Phase 4 Components: `intrinsics/generated.rs` Split (Landed Source Split)

**Current state:** `runtime/molt-runtime/src/intrinsics/generated.rs` is the canonical generated `INTRINSICS` manifest table and re-exports the composed resolver. `tools/gen_intrinsics.py` now also generates `runtime/molt-runtime/src/intrinsics/generated_resolvers/`, with one resolver module per category from `manifest.pyi` + `categories.toml`.

**End-state:** per-category resolver files are the stepping stone. The final crate-composition state moves category resolver ownership into the corresponding runtime leaf crate and leaves `molt-runtime` as a thin facade over leaf-owned sub-registries plus the combined manifest surface required by frontend/WASM tooling.

**Critical constraint:** The `INTRINSICS` constant stays in `generated.rs` for existing parser-facing tools (`src/molt/frontend/_types.py`, `src/molt/_wasm_runtime_exports.py`) and for registry iteration. Resolver address-taking lives in `generated_resolvers/`, so test/WASM `resolve_symbol` behavior stays intact while resolver implementation edits stop churning the manifest table.

**Tool change:** `tools/gen_intrinsics.py` always emits the split resolver tree, emits resolver arms in rustfmt-stable form, skips exact-content no-op writes before invoking rustfmt, lazy-loads memory-guard formatting custody only when a changed Rust file needs formatting, and formats changed generated Rust files through `MOLT_GENERATOR` custody. The guarded no-op generator path now avoids resolver-file mtime churn, avoids per-file rustfmt subprocesses, and measured at 0.65s on 2026-06-20 after the lazy guard-import cut.

**Impact:** resolver-body edits are localized to the touched category module. New manifest entries still update the canonical `INTRINSICS` table until the per-crate manifest composition step lands, but the address-taking resolver hub is no longer one source file.

### DX Component: Unified TIR dump ergonomics

**`/Users/adpena/Projects/molt/runtime/molt-ir/src/tir/printer.rs`**

Add `TirDumpConfig` struct:
```rust
pub struct TirDumpConfig {
    pub enabled: bool,
    pub func_filter: Option<String>,    // empty = all functions
    pub pass_filter: Option<String>,    // empty = all passes (pre/post)
    pub show_lir: bool,                 // MOLT_TIR_DUMP_LIR=1
}

impl TirDumpConfig {
    pub fn from_env() -> Self { ... }
    pub fn matches_func(&self, func_name: &str) -> bool { ... }
    pub fn matches_pass(&self, pass_name: &str) -> bool { ... }
}
```

`MOLT_TIR_DUMP` env-var syntax: `fn_name` alone, `fn_name:pass_name`, or `:pass_name` (all functions, specific pass). Replaces the current split between `TIR_DUMP=1` (printer.rs:18) and `MOLT_TIR_DUMP` (simple_backend.rs:2792). The `TIR_DUMP=1` spelling stays as a backward-compat alias.

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/pass_manager.rs`**

Change `MOLT_VERIFY_ANALYSIS` check at line 173 to:
```rust
let verify_analysis_filter: Option<String> = std::env::var("MOLT_VERIFY_ANALYSIS")
    .ok()
    .filter(|v| v != "0")
    .map(|v| if v == "1" { String::new() } else { v });
// verify_analysis_filter = None → disabled
// verify_analysis_filter = Some("") → all passes
// verify_analysis_filter = Some("gvn") → only after "gvn" pass
```

The post-pass check at line 382 becomes `if verify_analysis_filter.as_ref().map_or(false, |f| f.is_empty() || pass.name().contains(f.as_str()))`.

**`/Users/adpena/Projects/molt/runtime/molt-backend/src/main.rs`**

Add `MOLT_TIR_DUMP` and `MOLT_VERIFY_ANALYSIS` to `DAEMON_REQUEST_ENV_KEYS` at line 42 (currently `TIR_DUMP` is listed at line 47 but `MOLT_VERIFY_ANALYSIS` is missing).

## 4. Soundness Argument

**Phase 1 (LTO change):** Thin LTO is strictly a superset of no LTO and produces correct code for all Rust programs. The only risk is a regression in the Cranelift retry path that uses `catch_unwind`. The CLAUDE.md already documents why `panic = "unwind"` is required (line 292-309 of Cargo.toml). Thin LTO preserves unwind tables; fat LTO also preserves them (panic="unwind" is explicit). No miscompile risk.

**Phase 2 (function_compiler.rs split):** This is a structural move-only split within the same crate. No function signatures change. No data structures change. The `use super::*` pattern already used in `native_backend/mod.rs` and both sub-files means all imports remain visible. The only risk is a name collision if two sub-modules define a private helper with the same name — Rust's module system makes this a compile error, not a silent miscompile. The split is safe to verify incrementally: compile after moving each family, before the next.

**Phase 3 (crate extraction):** The key soundness property is that `molt-backend-native` depends on `molt-backend` (core), not vice versa. No circular dependency. The `extern "C"` ABI between the runtime and the backend is unchanged — that ABI lives in `molt-runtime-core/src/lib.rs` (the FFI declarations at line 377) and is independent of crate boundaries. The Cranelift types (`cranelift_codegen::ir::Value`, etc.) stay intra-crate in `molt-backend-native`. The cross-crate boundary between core and native is clean: `TirFunction`, `TirModule`, `SimpleIR`, `FunctionIR`, `TargetInfo` (all from core) flow into `SimpleBackend` (native). These types are already the established interface — the crate boundary just makes it explicit.

**Phase 4 (generated.rs split):** The landed source split keeps `generated.rs` as the canonical `INTRINSICS` manifest table and re-exports a composed resolver from `generated_resolvers/`. Moving each category resolver to its own generated file changes compilation unit boundaries but not behavior. The registry path that uses `resolve_symbol` stays intact through the composed resolver.

**Conservative-correct first-cut rule:** Phases 1-4 are all refactors. None of them add new optimization passes or new code paths. Any regression is a miscompile introduced by the refactor itself, which is detectable by the differential test suite (molt_diff.py basic/stdlib lanes) with zero behavior tolerance.

## 5. Legacy This Arc Deletes

| Legacy | Deleted By | Condition |
|--------|-----------|-----------|
| `function_compiler.rs` as a 38K-line monolith | Phase 2 split | Phase 2 landing: the monolith is replaced by the dispatcher + 8 sub-modules |
| Fat LTO on developer iteration profile | Phase 1 Cargo.toml change | Immediate on landing |
| `generated.rs` as a 12K+ line manifest+resolver file | Phase 4 split + gen_intrinsics.py update | Phase 4 landing: manifest table remains in `generated.rs`; per-category resolver bodies move to `generated_resolvers/` |
| Separate `TIR_DUMP` / `MOLT_TIR_DUMP` / `MOLT_DUMP_IR` env-var proliferation | Phase 1 DX unification | Unified under `MOLT_TIR_DUMP` with backward-compat aliases |
| `native_backend/` living inside `molt-backend` (forces TIR-edit → native recompile and native-edit → TIR recompile of same crate) | Phase 3 crate extraction | Phase 3 landing: editing TIR passes no longer recompiles Cranelift codegen units |

## 6. Test Plan

### Rust Unit Tests

**Phase 1 — LTO regression gate:**
- New test in `runtime/molt-backend/tests/`: `test_lto_profile_correctness.rs` — compiles a representative TIR function under `release-fast` (thin LTO) and verifies output bytewise matches the `release` reference. This test is a build-system test, not a runtime test; it verifies that the Cranelift object emitted is identical under thin vs fat LTO for a fixed function.
- Existing `tools/verify_native_binary_valid.sh` (the binary-size self-protection gate from commit ddc4ff73b) must pass after the LTO change.

**Phase 2 — function_compiler split:**
- No new behavioral tests needed — the split is a pure module reorganization. The test gate is: `cargo test -p molt-backend --features native-backend` must pass 882+ tests (current baseline) with 0 new warnings.
- Add a structural lint test: a Rust `#[test]` in `native_backend/mod.rs` that asserts `std::mem::size_of::<FunctionCompiler>()` equals the pre-split value, catching any accidental field reorder.

**Phase 3 — crate extraction:**
- `cargo test -p molt-backend-native --features native-backend` must pass the full test suite (same 882+ count — tests migrate with the code).
- `cargo test -p molt-backend --features native-backend` (core alone) must pass the TIR-only tests (parallel, pass_manager, analysis, bce, licm, gvn, inliner, etc.).
- New integration test: `tests/test_crate_boundary.rs` verifying that importing `molt_backend::tir::passes::run_pipeline` does NOT require `cranelift_codegen` to be in scope (i.e., the Cranelift feature is not accidentally leaked into the core crate).

**Phase 4 — generated.rs split:**
- `cargo test -p molt-runtime` must pass with 0 new warnings.
- New test in `runtime/molt-runtime/tests/`: assert that each `resolve_X_symbol` function only references symbols from its own feature domain (e.g., `resolve_crypto_symbol` only address-takes `molt_hash_*`, `molt_hmac_*` etc.).
- `python3 tools/gen_intrinsics.py` must produce identical `INTRINSICS` slice contents (same symbols, same order) when compared against the pre-split generated.rs.

### Differential Test Shapes

All differential tests run via `tests/molt_diff.py` with `--lane basic --lane stdlib` against CPython 3.12, 3.13, and 3.14.

**Bigint correctness (must stay correct across all phases):**
```python
# tests/differential/basic/build_dx_bigint_invariant.py
def apply(f, x, n):
    for _ in range(n):
        x = f(x)
    return x

result = apply(lambda x: x * 2, 1 << 60, 7)
assert result == 1 << 67
print(result)
```
Covers: `apply(f, 1<<60, 7)` — the canonical bigint-correctness oracle from the MEMORY.md (must stay bigint-correct, no trusted-unbox regression from Phase 3 crate boundary changes).

**Exception path correctness (must stay correct across all phases):**
```python
# tests/differential/basic/build_dx_exc_invariant.py
def f():
    try:
        raise ValueError("test")
    except ValueError as e:
        return str(e)
assert f() == "test"

def g(lst):
    return list(lst)

try:
    g(x for x in [1, 2] if (lambda: None)())
except TypeError:
    pass
print("ok")
```
Covers: exception-handling codegen path, the C2 fix (needs_exception_stack), iterator-consumer exception propagation (the original iter_consume_hang fix).

**Incremental build invariant (perf oracle):**
```python
# tests/differential/basic/build_dx_arith.py
def bench(n):
    total = 0
    for i in range(n):
        total += i
    return total
print(bench(10_000_000))
```
Covers: loop-IV accumulator (must not regress below CPython after Phase 1 LTO change, since thin LTO still enables cross-function inlining at import boundaries).

**Cross-backend parity (run on native + WASM after Phase 3):**
```python
# tests/differential/basic/build_dx_cross_backend.py
def fib(n):
    if n <= 1:
        return n
    return fib(n-1) + fib(n-2)
print(fib(30))
```
Run with `--target native`, `--target wasm`, `--target llvm` (where available). All three must produce 832040.

**Adversarial: trampoline edge case (Phase 2 fc_trampolines split must not regress):**
```python
# tests/differential/basic/build_dx_generator.py
def gen(n):
    for i in range(n):
        yield i * i

result = list(gen(5))
assert result == [0, 1, 4, 9, 16]
print(result)
```

## 7. Perf-Gate Plan

### Build-Time Benchmarks

These are measured via `tools/bench_backend_incremental.py` (confirmed to exist at that path). Add three new benchmark cases:

**BX-1: One-line edit to `fc_arith.rs` (post-Phase-2), incremental rebuild**
- Baseline (pre-Phase-2): edit any line in `function_compiler.rs` → measure time until `cargo build --profile release-fast -p molt-backend --features native-backend` exits.
- Target (post-Phase-2): same edit scope, same measurement. Expected delta: rebuild touches only `fc_arith.rs` compilation unit (~6K lines) instead of the 38K-line monolith. Expected improvement: 60-70% reduction in incremental rebuild time for arithmetic-family edits (from ~90s to ~30s under `release-fast` with `codegen-units=256`).

**BX-2: LTO phase time (Phase 1)**
- Measure: `cargo build --profile release-fast -p molt-backend --features native-backend --timings`. The LTO phase time appears in `cargo --timings` HTML output.
- Baseline: fat LTO serial phase. Expected: 30-60s for a clean rebuild.
- Target (thin LTO): parallel LTO. Expected: 5-15s LTO phase, with overall rebuild dropping from ~180s to ~90-120s (the 90-120s remainder is Cranelift's codegen time, which thin LTO does not accelerate).

**BX-3: Cross-worktree sccache hit rate (Phase 1)**
- Measurement: build agent 1 from clean, then build agent 2 with sccache shared. `sccache --show-stats` reports hit/miss.
- Target: >80% cache hit rate for non-backend-modified crates (cranelift-*, rayon, serde_json, etc.). These are dependency crates that change never — they should always hit after the first agent build.

**BX-4: TIR-edit incremental time (Phase 3)**
- Baseline: edit `tir/passes/gvn.rs` → rebuild time for the daemon binary.
- Pre-Phase-3: rebuilds `molt-backend` (includes Cranelift due to `cfg(feature="native-backend")` in the same crate).
- Post-Phase-3: rebuilds only `molt-backend` core (no Cranelift). `molt-backend-native` does not recompile (it depends on core; unchanged core does not force downstream recompile if only pass metadata changes in the rlib). Expected improvement: 40-50% reduction since Cranelift's codegen units (~25% of total compile time) are skipped.

### Runtime Benchmarks (must not regress)

Run `tools/bench.py` on all profiles (release-fast/dev-fast/debug-asserts) on all targets (native, WASM) after each phase:

| Benchmark | File | CPython gate |
|-----------|------|-------------|
| `bench_sum.py` | `/Users/adpena/Projects/molt/bench/bench_sum.py` | molt >= CPython |
| `bench_fib.py` | `/Users/adpena/Projects/molt/bench/bench_fib.py` | molt >= CPython |
| `bench_dict.py` | `/Users/adpena/Projects/molt/bench/bench_dict.py` | molt >= CPython |
| `bench_while.py` | `/Users/adpena/Projects/molt/bench/bench_while.py` | molt >= CPython |
| `bench_calls.py` | `/Users/adpena/Projects/molt/bench/bench_calls.py` | molt >= CPython |

Phase 1 (LTO change) is the only phase with a realistic runtime perf risk — thin LTO may reduce the quality of cross-crate optimization marginally. If bench_fib or bench_sum regress below CPython after Phase 1, the mitigation is to check whether the regression is in the daemon binary itself (acceptable — the daemon is a compiler, not user code) or in the compiled user binary (not acceptable). If the latter, `release-output` (fat LTO) is used for user binary compilation; `release-fast` (thin LTO) is used only for the daemon iteration loop.

## 8. Risk and Dependency Notes

### Blocked-By

- Phase 3 (crate extraction) is blocked until Phase 2 (function_compiler split) is stable. The split reduces the blast-radius of getting the crate boundary wrong.
- Phase 4 source splitting is no longer blocked. The remaining dependency is per-crate sub-registry ownership as runtime leaf crates take over their intrinsic implementations.
- Phase 3 is blocked-by Phase E e1 activation (currently HELD per MEMORY.md): the driver wiring of `run_module_pipeline` into production codegen depends on `SimpleBackend`'s call path, which Phase 3 moves to `molt-backend-native`. Land Phase 3 AFTER Phase E e1 is stable, or land Phase 3 first with a careful baton-pass that Phase E e1 needs to update import paths in `main.rs`.

### Unblocks

- Phase 1 (LTO change): immediately unblocks faster iteration for every ongoing arc. No ordering dependencies.
- Phase 2 (fc split): unblocks multiple agents editing different opcode families in parallel without serializing on the same file.
- Phase 3 (crate extraction): unblocks agents working on TIR passes from serializing against agents working on Cranelift codegen. These two workstreams (optimizer foundation vs native codegen quality) are the two highest-velocity lanes in the 5-year program.
- Phase 4 (generated.rs split): unblocks resolver-body edits from touching the full manifest table; the per-crate sub-registry step completes the compile-invalidation win for new intrinsic entries.

### Cross-Cutting Notes

- The `MOLT_SESSION_ID` / `CARGO_TARGET_DIR` isolation already works correctly (cli.py:12547-12573). No change needed.
- The 3-daemon-max enforcement is a session-level policy (cli.py:24972 region), not a build-system policy. It is unaffected by all phases.
- The `sccache` shared directory approach (Phase 1) requires that all agent worktrees resolve to the same project root. The existing `_session_target_dir` uses `project_root / "target" / "sessions" / sid` for the build artifact dir. The shared sccache dir uses `project_root / ".sccache"` — a parallel sibling. Agents write compiled rlibs to their isolated `target/sessions/<sid>/` but the sccache dir functions as a shared rlib cache. This is safe because sccache caches by content hash (compiler version + source hash + flags), not by path.
- The `CARGO_INCREMENTAL` setting: currently unset (defaults to 1 for dev profiles, 0 for release profiles). For `release-fast` with thin LTO, `CARGO_INCREMENTAL=0` is the correct choice (incremental compilation is incompatible with LTO — cargo already disables it automatically when lto != "off"). No change needed.
- The macOS fast-linker: `ld64.lld` is available via `llvm@21` which is already a dependency (MEMORY.md: "LLVM 21.1.8 keg-only at `/opt/homebrew/opt/llvm@21`"). The fast-linker config for macOS should use `/opt/homebrew/opt/llvm@21/bin/ld64.lld` when present.

### Rollback

- Phase 1: revert two Cargo.toml profile lines. Zero risk.
- Phase 2: revert is reconstituting `function_compiler.rs` from the sub-modules. Because the split is a move-only refactor, the original file can be regenerated by concatenating the sub-modules in order. A pre-Phase-2 git tag provides the exact content.
- Phase 3: the most complex rollback. Reverting means moving `native_backend/` back into `molt-backend` and removing `molt-backend-native` from the workspace. The `main.rs` import path changes revert. This is a ~30-minute operation if needed. Mitigation: land Phase 3 with an integration test that verifies the crate boundary produces byte-identical output (the same compiled Python program produces the same object file on the same commit).

## 9. Phased Landing Sequence

Each phase is a complete structural piece that can land independently and leave the codebase in a fully-green state.

### Phase 1a: LTO profile split + `release-output` profile (1 day)

- [ ] Modify `/Users/adpena/Projects/molt/Cargo.toml` line 295: `lto = "fat"` → `lto = "thin"` in `[profile.release-fast]`.
- [ ] Add `[profile.release-output]` block with `lto = "fat"`, `opt-level = 3`, `codegen-units = 1`, `panic = "unwind"`, `strip = "symbols"`.
- [ ] Update CLAUDE.md build instruction to note that the shipped daemon uses `release-output`; developer iteration uses `release-fast`.
- [ ] Measure `cargo --timings` output for a clean `release-fast` build; record LTO phase duration before and after for BX-2.
- [ ] Run full differential: `python3 tests/molt_diff.py --lane basic --lane stdlib`. Must be byte-identical to pre-change.
- [ ] Run `tools/verify_native_binary_valid.sh`. Must pass.
- [ ] `cargo test -p molt-backend --features native-backend`. Must pass 882+ tests, 0 new warnings.
- [ ] `git add Cargo.toml CLAUDE.md && git commit -m "build: split release-fast LTO (thin) from release-output (fat) — Phase 1a DX arc"`.

### Phase 1b: sccache shared directory + macOS fast linker (1 day)

- [ ] Modify `_maybe_enable_sccache` in `/Users/adpena/Projects/molt/src/molt/cli.py` (line 9361): add `SCCACHE_DIR` injection pointing to `project_root / ".sccache"` when sccache is found.
- [ ] Add `SCCACHE_CACHE_SIZE = "20G"` to the sccache env dict.
- [ ] Add `.sccache/` to `/Users/adpena/Projects/molt/.gitignore`.
- [ ] Create `/Users/adpena/Projects/molt/.cargo/config-fast-link-macos.toml` with `[target.aarch64-apple-darwin]` using the lld from `/opt/homebrew/opt/llvm@21/bin/ld64.lld`.
- [ ] Document in `.cargo/config.toml` comment block that `config-fast-link-macos.toml` is the macOS fast-linker opt-in.
- [ ] Add `MOLT_VERIFY_ANALYSIS` and `MOLT_TIR_DUMP` to `DAEMON_REQUEST_ENV_KEYS` in `main.rs` (currently `MOLT_VERIFY_ANALYSIS` is missing from the list at line 42).
- [ ] Test: two-agent sccache hit rate measurement (BX-3).
- [ ] `git add src/molt/cli.py .cargo/config-fast-link-macos.toml .gitignore runtime/molt-backend/src/main.rs && git commit -m "build: shared sccache dir + macOS fast linker opt-in — Phase 1b DX arc"`.

### Phase 1c: Unified TIR dump ergonomics (0.5 days)

- [ ] Add `TirDumpConfig` struct to `/Users/adpena/Projects/molt/runtime/molt-ir/src/tir/printer.rs` with `from_env()`, `matches_func()`, `matches_pass()`.
- [ ] Change `tir_dump_enabled()` to return `TirDumpConfig` (keep the boolean form as `TirDumpConfig::enabled` field for call-site compatibility).
- [ ] Update `simple_backend.rs` lines 2371 and 2791-2792 to use the new `TirDumpConfig`.
- [ ] Update `pass_manager.rs` line 173 to accept the filter form of `MOLT_VERIFY_ANALYSIS`.
- [ ] Verify: `MOLT_TIR_DUMP=fib:gvn cargo test ...` prints TIR only for the `fib` function after the GVN pass.
- [ ] `git add runtime/molt-ir/src/tir/printer.rs runtime/molt-passes/src/tir/pass_manager.rs runtime/molt-backend/src/native_backend/simple_backend.rs && git commit -m "build: unified MOLT_TIR_DUMP + MOLT_VERIFY_ANALYSIS filter — Phase 1c DX arc"`.

### Phase 2: `function_compiler.rs` module split (3 days)

- [ ] Create `runtime/molt-backend/src/native_backend/fc_arith.rs` — move arithmetic op handlers from `function_compiler.rs`.
- [ ] Create `runtime/molt-backend/src/native_backend/fc_control.rs` — move control flow handlers.
- [ ] Create `runtime/molt-backend/src/native_backend/fc_collections.rs` — move collection ops handlers.
- [ ] Create `runtime/molt-backend/src/native_backend/fc_exceptions.rs` — move exception handling.
- [ ] Create `runtime/molt-backend/src/native_backend/fc_closures.rs` — move closure/call/RC emission.
- [ ] Create `runtime/molt-backend/src/native_backend/fc_trampolines.rs` — move trampoline emission (currently split between `simple_backend.rs` `TrampolineKey` and `function_compiler.rs` emission).
- [ ] Create `runtime/molt-backend/src/native_backend/fc_loops.rs` — move `scan_loop_hoistable_lists` (currently function_compiler.rs:48) and loop-index codegen.
- [ ] Create `runtime/molt-backend/src/native_backend/fc_async.rs` — move state-machine codegen.
- [ ] Update `native_backend/mod.rs` to declare all new sub-modules.
- [ ] Update `function_compiler.rs` (residual) to be only the `FunctionCompiler` struct, `new()`, `compile_function()`, and the `emit_op()` dispatch that calls into the sub-modules.
- [ ] After each sub-module: `cargo build -p molt-backend --features native-backend` must succeed, `cargo test -p molt-backend --features native-backend` must pass.
- [ ] Measure BX-1 (incremental rebuild time for a one-line fc_arith.rs edit).
- [ ] Full differential parity check.
- [ ] `git add runtime/molt-backend/src/native_backend/ && git commit -m "build: split function_compiler.rs into 8 opcode-family sub-modules — Phase 2 DX arc"`.

### Phase 3: `molt-backend-native` crate extraction (4 days)

- [ ] Create `/Users/adpena/Projects/molt/runtime/molt-backend-native/Cargo.toml` with the crate definition.
- [ ] Create `/Users/adpena/Projects/molt/runtime/molt-backend-native/src/lib.rs` as the new entry point re-exporting `SimpleBackend`, `CompileOutput`, `NativeBackendModuleContext`.
- [ ] Move `runtime/molt-backend/src/native_backend/` → `runtime/molt-backend-native/src/native_backend/`.
- [ ] Move `runtime/molt-backend/src/llvm_backend/` → `runtime/molt-backend-native/src/llvm_backend/`.
- [ ] Move `runtime/molt-backend/src/native_backend_consts.rs` → `runtime/molt-backend-native/src/`.
- [ ] Remove `#[cfg(feature = "native-backend")] mod native_backend;` and related `pub use` re-exports from `runtime/molt-backend/src/lib.rs`.
- [ ] Remove `native-backend` feature from `runtime/molt-backend/Cargo.toml` `[features]`; move Cranelift deps to `molt-backend-native/Cargo.toml`.
- [ ] Update `Cargo.toml` workspace `members` to include `"runtime/molt-backend-native"`.
- [ ] Update the daemon `main.rs` (currently in `molt-backend`) — move it to `molt-backend-native/src/bin/molt-backend-daemon.rs` with updated imports.
- [ ] Add `[profile.release-fast.package.molt-backend-native]` to root `Cargo.toml` matching the pattern at lines 314-331.
- [ ] Update CLAUDE.md build command: `cargo build --profile release-fast -p molt-backend-native --features native-backend`.
- [ ] After the move: `cargo build -p molt-backend --features native-backend` (core only, no Cranelift) must succeed.
- [ ] `cargo build -p molt-backend-native --features native-backend,llvm` must succeed.
- [ ] `cargo test -p molt-backend-native --features native-backend` must pass 882+ tests.
- [ ] `cargo test -p molt-backend --features native-backend` (core tests only) must pass TIR-only tests.
- [ ] Measure BX-4 (TIR-edit incremental time).
- [ ] Full differential parity check.
- [ ] Run `tools/bench.py` on all benchmarks. No runtime regression vs pre-Phase-3 baseline.
- [ ] `git add runtime/molt-backend/ runtime/molt-backend-native/ Cargo.toml && git commit -m "build: extract molt-backend-native crate — Cranelift/LLVM isolated from TIR core — Phase 3 DX arc"`.

### Phase 4: `intrinsics/generated.rs` Split

- [x] Modify `tools/gen_intrinsics.py` to generate per-category resolver files under `runtime/molt-runtime/src/intrinsics/generated_resolvers/`.
- [x] Keep `generated.rs` as the canonical `INTRINSICS` manifest table plus thin resolver re-export, preserving existing parser-facing tools.
- [x] Run `uv run python tools/guarded_exec.py --prefix MOLT_GENERATOR --timeout 300 -- uv run python tools/gen_intrinsics.py`.
- [x] Run `cargo check --manifest-path runtime/Cargo.toml -p molt-runtime --lib`.
- [x] Run `cargo check --manifest-path runtime/Cargo.toml -p molt-runtime --lib --no-default-features --features stdlib_micro`.
- [x] Run `cargo check -p molt-backend --profile dev-fast`.
- [ ] Continue from source-file split to per-crate sub-registries as runtime leaf extraction lands.

## Rebuild Dependency Graph (Post-All-Phases)

```
molt-obj-model (stable, rarely changed)
    └── molt-runtime-core (GIL vtable, FFI declarations)
            └── molt-runtime-{crypto,net,asyncio,math,...} (leaf crates, parallel)
                    └── molt-runtime (facade + builtins/object/ + intrinsics/)
                                      ^
                    molt-backend (core: tir/, ir.rs, passes.rs, representation_plan.rs, wasm.rs)
                            └── molt-backend-native (native_backend/ + llvm_backend/)
                                        └── molt-backend-daemon (main.rs binary)
```

Under this graph:
- Editing a TIR pass in `molt-backend/src/tir/passes/gvn.rs` recompiles `molt-backend` core only; `molt-backend-native` recompiles only if `molt-backend`'s rlib metadata changed.
- Editing Cranelift codegen in `molt-backend-native/src/native_backend/fc_arith.rs` recompiles only the `fc_arith.rs` compilation unit within `molt-backend-native`; `molt-backend` core does not recompile.
- Editing a builtin in `molt-runtime/src/builtins/functions_re.rs` recompiles only `molt-runtime`; neither backend crate recompiles.
- Editing `molt-runtime-crypto` recompiles only that leaf crate and `molt-runtime` (the facade dep).

This graph is what makes parallel agent development correct: agents working on TIR optimization (gvn, licm, bce, scev, inliner), agents working on native codegen (fc_arith, fc_closures, trampolines), and agents working on runtime builtins (str, list, dict, exceptions) operate on fully disjoint compilation units.

## Quantified Expected Improvements

| Metric | Current | Target | Lever |
|--------|---------|--------|-------|
| `release-fast` clean build time (daemon binary) | ~5 min | ~3 min | Phase 1 (thin LTO parallel phase) |
| Incremental rebuild after fc_arith.rs edit | ~90s | ~25s | Phase 2 (per-opcode compilation unit) |
| Incremental rebuild after gvn.rs edit | ~90s | ~45s | Phase 3 (TIR core doesn't recompile Cranelift) |
| Incremental rebuild after builtins/str.rs edit | ~120s | ~60s | Existing decomposition; no change here (molt-runtime still monolithic body) |
| Cross-agent sccache hit rate (cranelift deps) | ~0% | ~85% | Phase 1b (shared .sccache dir) |
| TIR dump: filter to one function+pass | not supported | supported | Phase 1c |
| LTO phase wall time in incremental build | ~30-60s | ~5-15s | Phase 1 (thin LTO) |
