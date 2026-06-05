<!-- Phase 3 (THE keystone) extraction baton — produced by agent-dx after Phase 0 measurement
     + full static boundary verification. NOT YET EXECUTED. Blocked this session by a hard
     constraint (partner active in llvm_backend/**); see §0. Everything here is verified against
     base commit 9e93503bb. -->

# Phase 3 Baton: extract `molt-backend-native` crate

## §0. Why this is a baton, not a landed change (the honest blocker)

A crate extraction is an ATOMIC structural change: you cannot land "half a crate boundary"
without leaving two parallel sources of truth (the exact compound-interest-of-bugs trap CLAUDE.md
forbids). The full extraction REQUIRES relocating `runtime/molt-backend/src/llvm_backend/` into the
new crate (blueprint 08, line 457). At session time, **`llvm_backend/lowering.rs` was modified
08:45 the same day by an active partner** (and the task scope explicitly says "Do NOT touch
llvm_backend/**"). Moving partner-active files would trample in-flight work (CLAUDE.md Git
Discipline: "NEVER trample partner work"). Therefore the correct call is: land the measured,
independent Phase 1a win (thin LTO, ~41%), and hand Phase 3 off with a complete, de-risked plan to
execute once the llvm_backend arc settles. **Do NOT land a half-extracted hybrid.**

## §1. Why Phase 3 is THE lever (measured)

From `dx_baseline.md`: editing ANY molt-backend source — a `tir/passes/*.rs` optimizer pass OR a
`native_backend/*.rs` codegen file — costs the SAME ~125–168 s, because cargo's recompilation unit
is the CRATE. The daemon-bin fat→thin LTO (Phase 1a) cut that to ~81 s, but it is STILL a full
`molt-backend` rebuild every time. Phase 3 makes a `tir/passes` edit recompile only `molt-backend`
*core* (no Cranelift codegen units, no 34K-line `function_compiler.rs`, no daemon-bin re-LTO if the
core rlib's metadata is unchanged), and vice versa. This is the structural decoupling the lib.rs
module-split (34e3bddbf) could never achieve (modules are not recompilation boundaries; crates are).

## §2. The boundary is CLEAN (verified — no dependency cycle)

`tir/`, `passes.rs`, `ir.rs`, `ir_rewrites.rs`, `representation_plan.rs` have **ZERO code-level refs
to `native_backend`** (only doc-comments / one test-name string in `module_phase.rs:43`,
`call_graph.rs:8`, `bce.rs:6`, `tests_roundtrip.rs:1248`). So core→native is the only direction;
extraction has no cycle to untangle.

## §3. What MOVES to `molt-backend-native` vs STAYS in `molt-backend` (core)

### Moves (native crate):
- `runtime/molt-backend/src/native_backend/` (whole dir; incl. the 39K-line `function_compiler.rs`,
  `simple_backend.rs`, `vec_layout.rs`, `mod.rs`).
- `runtime/molt-backend/src/llvm_backend/` (whole dir; feature `llvm`). **<-- partner-active; the
  gating reason this is a baton.**
- `runtime/molt-backend/src/native_backend_consts.rs` (the NaN-box tag constants `QNAN`, `TAG_*`,
  shared by native AND llvm — both move together).
- `runtime/molt-backend/src/main.rs` (the 158 KB daemon) -> `molt-backend-native/src/bin/molt-backend.rs`.
  It is the integration point that imports BOTH core (`SimpleIR`, `luau::LuauBackend`,
  `rust::RustBackend`, `wasm::WasmBackend`, `rewrite_annotate_stubs`) AND native (`SimpleBackend`).
  It must live in the crate that can see both = the native crate (which depends on core).
- `runtime/molt-backend/src/json_boundary.rs` — used by `main.rs` (`use crate::json_boundary::{...}`).
  EITHER move it with the daemon OR make it `pub` in core. Recommend: make `pub` in core (it is a
  pure transport type, also useful to core tests). Verify no other core module needs it private.
- `runtime/molt-backend/src/bin/typed_repr_report.rs` — imports `molt_backend::tir::*` + `SimpleIR`
  + `rewrite_annotate_stubs` (all core). It can stay a bin of EITHER crate; cleanest is to keep it a
  core bin (it only needs core symbols). Confirm its imports after the move.

### Stays (core `molt-backend`):
- `tir/` (all 73 files: passes, analysis, pass_manager, module_phase, parallel, lower_*, target_info,
  serialize, cache, verify_*, type_refine, printer, CallGraph, lir, ops, types, values).
- `ir.rs`, `ir_rewrites.rs`, `ir_schema.rs`.
- `representation_plan.rs` (the Repr lattice — int-carrier source of truth).
- `passes.rs` (SimpleIR passes — NO Cranelift dep).
- `intrinsic_symbols.rs`, `debug_artifacts.rs`, `egraph_simplify.rs`.
- `wasm.rs`, `wasm_imports.rs` (feature `wasm-backend` — NO Cranelift dep, stays).
- `luau.rs`, `luau_ir.rs`, `luau_lower.rs` (Luau transpiler — stays).
- `rust.rs` (Rust transpiler — stays).
- `lib.rs` (becomes core-only; drops the `mod native_backend` + native re-exports).

## §4. The cross-crate pub interface (exact, minimal — verified by grep of native's `crate::` refs)

### Core must expose `pub` (currently `pub` already, unless noted) for native to consume:
- `tir::*`: `TirModule`, `TirFunction`, `passes::run_pipeline`, `run_module_pipeline`,
  `lower_from_simple::{lower_to_tir, lower_functions_to_tir_module}`,
  `lower_to_simple::{lower_to_simple_ir, validate_labels}`, `lower_to_lir::lower_function_to_lir`,
  `type_refine::refine_types`, `target_info::{TargetInfo, SimdCaps}`,
  `serialize::{serialize_ops, deserialize_ops}`, `verify_lir::verify_lir_function`,
  `verify_lir_repr::verify_register_passable`, `printer::print_function`, `CallGraph::build`,
  `cache::{CompilationCache, backend_cache_dir}`. (Most already `pub`; audit each.)
- `passes::{ReturnAliasSummary, compute_return_alias_summaries, compute_rc_coalesce_skips}` — make `pub`.
- `debug_artifacts::{append_debug_artifact, write_debug_artifact}` — already `pub mod`.
- `representation_plan::{Repr, LlvmReprFacts}` — `Repr` already re-exported `pub`; make `LlvmReprFacts` `pub`.
- `ir_rewrites::rewrite_phi_to_store_load` — already `pub`.
- `SimpleIR`, `rewrite_annotate_stubs` — already `pub`.
- **`TrampolineKind` / `TrampolineSpec`** (defined in `lib.rs:219/231` as `pub(crate)`): used by BOTH
  `wasm.rs` (core, stays) AND native. KEEP in core, change `pub(crate)` -> `pub`. (Narrow, clean.)
- `pending_bits()`, `stable_ic_site_id()` (lib.rs, `pub(crate)`, gated on native|llvm): these are used
  by native — move them INTO the native crate (they are native/llvm-only helpers), or `pub` in core.
  Prefer MOVE (they reference `native_backend_consts` which moves anyway).

### Native crate's own internals (move WITH native, NOT a cross-crate interface):
`NanBoxConsts`, `VarValue`, `DeferredDefine`, `switch_to_block_tracking`, `block_has_terminator`,
`unbox_int`, `extend_unique_tracked` — ALL defined in `native_backend/simple_backend.rs`. They are
currently re-exported through `lib.rs` as `pub(crate)` only so `llvm_backend` (also moving) and the
daemon (also moving) can see them. After extraction they are crate-internal to `molt-backend-native`
(`pub(crate)` within it). The `lib.rs` re-exports at lines 25–30 are DELETED.

## §5. Mechanical steps (atomic — all in ONE commit)

1. `runtime/molt-backend-native/Cargo.toml`:
   ```toml
   [package] name = "molt-backend-native"; version = "0.1.0"; publish = false; edition = "2024"
   [dependencies] molt-backend = { path = "../molt-backend", default-features = false }
   cranelift-codegen/-frontend/-module/-object/-native = { ... optional = true }   # MOVED from molt-backend
   inkwell = { version = "0.8", features=["llvm21-1"], optional = true }           # MOVED
   libc, rayon, serde, serde_json, rmp-serde  # as needed by moved code
   [features] default=["native-backend"]; native-backend=[dep:cranelift-*]; llvm=["dep:inkwell"]; ...
   ```
2. `git mv` the four source trees/files in §3 into `molt-backend-native/src/`.
3. New `molt-backend-native/src/lib.rs`: `mod native_backend; #[cfg(feature="llvm")] pub mod llvm_backend;
   mod native_backend_consts; pub use native_backend::{SimpleBackend, CompileOutput, NativeBackendModuleContext};`
4. `molt-backend/src/lib.rs`: DELETE `mod native_backend;`, the native re-exports (25–30),
   `mod native_backend_consts;` (moves), `pub mod llvm_backend;`, and the native|llvm-gated
   `pending_bits`/`stable_ic_site_id` (move). Make `TrampolineKind/Spec` `pub`. Make `passes::{...}`
   + `LlvmReprFacts` `pub`. Move/`pub` `json_boundary`.
5. `molt-backend/Cargo.toml`: REMOVE `native-backend` + `llvm` features and the cranelift/inkwell deps
   (they move to native). Keep `wasm-backend`, `luau-backend`, `rust-backend` (no Cranelift).
6. Root `Cargo.toml`: add `"runtime/molt-backend-native"` to `members`. Add
   `[profile.release-fast.package.molt-backend-native]` mirroring the molt-backend overrides
   (opt-3, cgu=256) + the cranelift-codegen/regalloc2 opt-3 overrides (already global, fine).
7. `git mv runtime/molt-backend/src/main.rs runtime/molt-backend-native/src/bin/molt-backend.rs`;
   fix its imports (`crate::json_boundary` -> wherever json_boundary lands).
8. **cli.py**: the daemon build (cli.py:25081) `--package molt-backend` -> `--package molt-backend-native`;
   artifact path resolution (cli.py:10334-10359, `_session_target_dir`, `17407`) — the bin is still named
   `molt-backend` (set `[[bin]] name = "molt-backend"` in the native crate so the artifact path is stable
   and NO other cli.py path logic changes). Verify `_DEFAULT_BACKEND_FEATURES` still maps.
9. CLAUDE.md build line: `cargo build --profile release-fast -p molt-backend-native --features native-backend`.

## §6. Gates for Phase 3 (must all pass in the landing commit)

- `cargo build -p molt-backend` (core, NO features) succeeds WITHOUT cranelift/inkwell in scope.
- `cargo build -p molt-backend-native --features native-backend` succeeds; `--features native-backend,llvm` succeeds.
- Feature matrix: core `wasm-backend`, `luau-backend`, `rust-backend` still build on `molt-backend`.
- `cargo test -p molt-backend-native --features native-backend` passes the migrated suite (the count
  that is currently in molt-backend's native tests); `cargo test -p molt-backend` passes the TIR-only tests.
- New `tests/test_crate_boundary.rs`: importing `molt_backend::tir::passes::run_pipeline` does NOT pull
  `cranelift_codegen` into scope (proves the feature isn't leaked into core).
- clippy `-D warnings` 0; compliance 46/46; e2e byte-identical (`apply(f,1<<60,7)`, fib(30), generator).
- **BX-4 measurement**: touch `tir/passes/gvn.rs` -> rebuild; confirm `molt-backend-native` does NOT
  recompile (only core + relink). Touch `native_backend/function_compiler.rs` -> confirm core does NOT
  recompile. Record both deltas vs the Phase-1a `~81 s` baseline.

## §7. Risk + rollback

- Risk: a core symbol native needs is missed -> compile error (loud, not silent). Fix by `pub`-ing it.
  This is iterative but safe (no miscompile class).
- Risk: `main.rs`/cli.py artifact-path drift -> daemon not found. Mitigate by keeping `[[bin]] name="molt-backend"`.
- Rollback: `git mv` back + revert lib.rs/Cargo.toml/cli.py. ~30 min. Land with the BX-4 + boundary test
  so regression is detectable.
- Order: land AFTER the llvm_backend partner arc is at a commit boundary (so the `git mv` of
  `llvm_backend/` does not collide). Coordinate; or split llvm out in a follow-up if it is still hot
  (native-only extraction first is possible IF `llvm_backend` temporarily stays in core behind a
  `molt-backend/llvm` feature that re-exports native's NaN-box consts — but that re-creates a
  cross-crate cycle, so it is NOT recommended; wait for llvm to settle and move both together).
