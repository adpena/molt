# Give both Cranelift code owners the final TIR body

W3 review proposal for HF-889, based on 4d0451c393. The integrator owns the review. The existing [walker design](../docs/design/foundation/codegen_walker.md) owns the architecture. This proposal specifies the change at the native function boundary and the frame and task operations needed to make it work.

## What a developer gets

A developer who builds with the dev profile or runs the JIT gets the same late integer, float and call expansions that LLVM release builds consume. HF-889 records 177 versus 8.06 instructions per iteration for the Cranelift and LLVM counter loops, and 374 versus 15.0 for integer arithmetic. These are historical observations. The switch has no measured result yet.

Today the native pipeline already produces terminal TIR, including specialization clones and explicit ownership operations. `NativeFinalFunctions::from_final_tir` projects that body into a mutable `FunctionIR` and a name-indexed representation plan, then drops the typed body. Linking an executable or serving the JIT both call `emit_cranelift_program`. Replacing its input fixes both entry points.

## Where the switch can land

Slot-06 recorded the answer to walker design question 4 in 8b9495450e. The AOT caller in `simple_backend/compile_driver.rs` and the JIT caller in `simple_backend/jit.rs` confirm it on this base. A JIT-first cut still needs every legal operation, ABI declaration and finalization step. A per-op cut would require crossings between two incompatible SSA and frame owners within one function. Switching only the target flag would expose late primitives to the existing SimpleIR consumer. Switch both callers and remove the old emitter in one commit. Frame, task and terminator extractions can land independently while LLVM exercises each in production.

Reuse Cranelift block parameters, the existing LLVM phi adapter, `FunctionCx`, the value table and the ABI generators. The precedent for isolating machine operations from a typed IR visitor is rustc's SSA builder interface. Cranelift's frontend already supports sealing the blocks after all predecessor edges have been emitted. The walker design and its machine differential record the concrete Molt trade-offs. [rustc builder interface](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_codegen_ssa/traits/builder/trait.BuilderMethods.html) · [Cranelift FunctionBuilder](https://docs.rs/cranelift-frontend/latest/cranelift_frontend/struct.FunctionBuilder.html)

## Global invariants

1. **One body and one function identity.** Each definition owns its terminal `TirFunction` through emission. Each declaration uses its frozen `NativeFunctionLinkageAbi` and has no executable body. Use the document's `FunctionId` and the existing table/remapping operations for filtering, clones, externalization and partitioning. Recover link names only at object/JIT symbol boundaries. Preserve the roots introduced by module registry data and callable manifests. A body and each reference to it must survive or remap together.
2. **One machine signature.** Caller and provider agree on parameter order, hidden execution context, ownership, and raw, boxed or void return. Use the frozen pre-partition linkage row for both declarations. Convert edge operands before branching with the shared coercion. Walker inputs carry `ValueId`, `BlockId`, `HeldCarrier` and TIR types. A missing definition, return mismatch or illegal opcode ends compilation before an object is finalized or a JIT address is published.
3. **One frame transition per edge.** Synchronous entry uses the existing frame shape and ordered homes. Stateful entry links the persistent activation record on resume and unlinks it on suspension or return. A runtime-linked frame follows its own ABI. Initialize the entry slot to zero and clear it before pop can release homes and re-enter Python. A failed entry has zero base. Preserve handle lifetimes, the exit path for an empty/exceptional return, and the caller's returned owner.
4. **One task layout and resume dispatch.** Resolve a runtime poll key or declared compiled entry and frame-size relocation once from typed identity. Preserve the source task kind, completion mode, captured function/context and payload custody through `TaskConstructorLayout` and the existing task generator. Read the saved state from the activation. State zero uses the default edge, and each resumed edge supplies its own block arguments. Pending-sentinel comparison, exception confirmation and the async-work poll are distinct operations.
5. **One confirmed pending read.** For the semantic operation currently named `emit_exception_pending_word`, zero bypasses the runtime. A nonzero byte requires confirmation from `RuntimeFn::ExceptionPending`; a stale byte can confirm clear. Preserve poll operations that also service async work or fetch an exception payload according to their own contracts.
6. **One production dispatch.** The shared walk covers the admitted final TIR for both native builders. AOT and JIT differ in module allocation, relocation, publication and lifetime. Both invoke the same Cranelift body emitter. Retire each backend-private optimization only after its benefit has been attributed and preserved or disproved on matching inputs. Use exhaustive Rust matches and the actual feature builds to check structural coverage.

## Implementation and review boundaries

Start with the frame generators already in `molt-codegen-abi`. Move frame-entry selection, ownership and cleanup into the walker. Keep LLVM helper placement where inlining preserves the caller CFG. Cranelift can emit the same generator in the caller. Extend the builder only for machine primitives needed by an extracted body, including access to the current execution record. Move closure slots, state loads/stores, yield, yield-from, waits, task creation and resume dispatch with their callers. Keep the layout computation in its existing ABI owner. Decode any remaining poll or task-kind spelling once at ingest, including runtime-native polls used by allocation. A compiled poll defined in another object must import that provider's size word so spill storage is large enough.

At this base, `Return`, `StateDispatch`, frame slot cleanup and the pending read live in **slot-06's reserved** `value_materialization.rs`. Its callable sibling also emits empty returns. The integrator must assign the call-site cut or arrange it with slot-06 before either lane changes these functions. W3 can implement the frame and state leaves independently after design review. The switch consumes slot-06's complete walk and exhaustive operation dispatch.

For the switch, adopt terminal TIR into the existing final-function owner. Use dense document IDs tied to the final function table. Preserve independently compiled object declarations, imported callables, closure/environment metadata, program slots, module registry relocations and JIT runtime resolution. Thread frozen linkage through module finalization, which currently passes `linkage: None` on the native path. Drop source op arrays as soon as ingest and partition metadata no longer need them. Avoid retaining two stdlib-sized programs. Count emitted terminal operations for diagnostics that describe machine emission. Keep source counts explicitly identified where a public diagnostic measures source operations.

`lower_to_simple_ir_with_names` currently feeds native `ScalarRepresentationPlan::project_final_tir`. Delete that consumer with the switch. `pipeline_cache.rs` also projects bodies for warm and cold SimpleIR requests and partitioning; use the existing TIR-only route and preserve bounded splitting and remapping before terminal drops. The requested rewritten-IR dump in `program_pipeline.rs` and round-trip tests in the pass crates are independent projection consumers. Retain projection where those consumers still request it. The shipped LLVM wasm32 route alone does not establish a WASM machine-code dependency on SimpleIR.

### Rust skeleton

The signatures below describe the ownership boundaries to review. `B` implements the existing `CodegenBuilder`. The function table retains the declaration ABI for each row. An execution-frame handle is absent when the function owns no frame, and its zero-initialized slot can hold a failed-entry base.

~~~rust
/// I1, I2. Store the definition in the same row as its frozen ABI. The row
/// index is the document FunctionId. Declarations have no machine body.
enum FinalRow {
    Declaration(NativeFunctionLinkageAbi),
    Definition { abi: NativeFunctionLinkageAbi, body: TirFunction },
}

/// I3. The slot holds the currently owned base. None for kind denotes an
/// ABI-linked frame, as in the existing frame-record API.
struct OwnedFrame<S> { slot: S, kind: Option<FrameEntryKind> }

/// I2, I3. Requires the current function's frozen ABI and held carriers.
/// Ensures the result is coerced before retiring the frame and returning.
/// Raises a fatal lowering diagnostic if a value would cross a void ABI.
fn emit_return<B: CodegenBuilder>(
    cx: &mut FunctionCx<'_, B>, b: &mut B, abi: &NativeFunctionLinkageAbi,
    value: Option<ValueId>, frame: Option<&OwnedFrame<B::Slot>>,
) { todo!() }

/// I4. Resolve the typed poll through the current document or runtime
/// manifest. The compiled variant covers a local definition and an import.
/// Only the linkage adapter materializes a symbol/relocation.
enum PollTarget { Runtime(RuntimeFn), Compiled(FunctionId) }

/// I5. Requires the current activation's thread record. Ensures a word of
/// zero or one and confirms every nonzero byte through the runtime reader.
/// Runtime confirmation can clear stale task state.
fn exception_pending_word<B: CodegenBuilder>(b: &mut B, thread: B::Word) -> B::Word {
    todo!()
}

impl SimpleBackend {
    /// I1, I2, I6. Requires complete rows and frozen module declarations.
    /// Ensures verified definitions and metadata precede publication by the
    /// code owner. Raises the existing backend diagnostic on failure.
    fn emit_cranelift_program(
        &mut self, program: NativeCodegenProgram, timing: bool,
    ) -> (usize, usize) { todo!() }
}
~~~

The shared walk and top-level dispatch signatures belong to slot-06. Backend-only strings are restricted to diagnostic text, debug names and symbols at linkage. Machine values keep the builder's associated types. Serialized field names and CLI choices remain boundary syntax. The typed TIR op or canonical ABI layout must carry their decoded meanings to the walker.

## Qualification

| Obligation | Independent oracle and failure it detects |
|---|---|
| Frame and return | Run existing ABI generator tests and walker machine kernels. Observe nested entry/pop, allocation failure, re-entry during home release, empty and typed returns. Compare Python-visible tracebacks, locals, return values and cleanup with the managed CPython oracle. |
| Tasks and exceptions | Exercise first entry, resume with different edge arguments, send/throw/close, yield-from, cancellation and live locals across suspension. Use a stale nonzero pending byte as well as a real exception and a clear byte. Verify both return value and runtime confirmation count. Use two jobs for async and GC. |
| Linkage and custody | Build producer and caller in separate objects with raw, boxed and void returns and a compiled poll whose spills enlarge its frame. Exercise the JIT loader and owner lifetime. Invalid declarations must fail before publication. Test warm and cold TIR cache inputs, split bodies, clones and extern rows. |
| Family extraction | Compare the 38 release binaries' LLVM text against the same base on the same workload. Where the brief's time exception applies, use interleaved retired instructions and equal output. Exercise model and actual Cranelift kernels and the declared Rust feature cells. |
| Production switch | Run dev/Cranelift basic and stdlib differentials against main, disposition each difference, and run JIT tests. Measure cold and warm user build latency, peak process-tree memory and executable output. Interleave the eight W3 benchmarks against main for instructions and elapsed time. Investigate any regression over 2% in shared lowering before landing. |

The eight workloads are bench_fib, bench_calls, bench_sum, nbody, spectral_norm, fannkuch, mandelbrot and bench_method_defaults. Build base and head at equal-length paths and execute each from the same path. Before deleting Cranelift lowering, obtain the integrator's comparative corpus and investigate each Cranelift advantage, including raise_catch and str_build. Match source/disassembly, a sampled per-module profile and interleaved counts before assigning cause. Record counter and runtime calls per iteration for changed hot paths. Update HF-889 on each implementation landing. These proofs remain outstanding.

## GoldfishContext

Paths are relative to the repository. Brace groups denote individual siblings.

- **Native pipeline and representation** — `runtime/molt-backend-native/src/native_backend/simple_backend/{compile_driver,jit,program_pipeline,module_metadata,linkage_carriers,trampolines}.rs`, its `tests.rs` and `tests/{tir_analysis,module_metadata,trampolines}.rs`; `runtime/molt-tir/src/{representation_plan.rs,representation_plan/test_fixtures.rs,tir/pipeline_cache.rs}`; `runtime/molt-ir/src/{function_table,linkage_abi,code_slots}.rs` and `tir/{target_info,ops,call_targets}.rs`. Direct-codegen fixtures currently accept SimpleIR. Convert them to the real final-body boundary and keep frontend transport tests at ingest.
- **Walker, frame and state** — `runtime/molt-backend-native/src/codegen_ssa/{mod,builder,function,control,custody,value_table,scalar,differential}.rs`; `scalar.rs` contains the interpreting builder. Compare both production builder impls in `native_backend/codegen_builder.rs` and `llvm_backend/lowering/codegen_builder.rs`. Inspect `llvm_backend/lowering.rs`, `lowering/{frame_record,state_machine_ops,task_site,runtime_helpers,value_materialization,op_dispatch}.rs`, `lowering/preserved_ops/callable_ops.rs` and `lowering/tests/{mod,runtime_declarations,preserved_ops}.rs` under the native crate. The latter call sites include other-lane custody. Existing ABI generators and model fakes live in `runtime/molt-codegen-abi/src/{frame_record,task_site,thread_record}.rs` and `{frame_record,task_site}/tests.rs`.
- **Switch and deletion** — `runtime/molt-backend-native/src/native_backend/{mod,simple_backend,function_compiler}.rs`, `function_compiler/fc/`, its tests including `list_index_fast_path.rs`, and the TIR facts and target selectors in `runtime/molt-passes/src/tir/{drop_phase,module_phase,pass_manager,passes/type_flow/mod}.rs`. Review `runtime/molt-ir/src/tir/op_kinds.toml` for target realization and the generated table only if its source contract changes. `tir_pipeline_target_fingerprint` already includes final lowering. Check old cache rejection and any persisted machine-code keys on the real warm path.
- **Path and registry consumers** — `tools/proof_plan.toml` has explicit op-kind-registry, native-callable-abi and pending-call-source-authority paths. Its generator `tools/gen_proof_plan.py` writes `.github/proof-plan.generated.json` and `docs/agent/PROOF_PLAN.generated.md`. `tools/{structural_audit.py,dx_build_timer.py,dead_code_allow_baseline.json}` and `tools/proof_queue_pkg/diagnostic_runtime_rules.py` name old paths. `src/molt/environment_registry.toml` declares existing readers/owners in the old emitter. Retire options with their last reader or move live diagnostics, then run `tools/gen_environment_registry.py` through the generator authority, including `src/molt/_environment_registry.py`. The switch introduces no environment variable. Search the full tree for the removed paths, imports, symbols and option identities at the actual landing base.
- **Existing proofs** — `tests/{test_gen_native_callable_abi,test_gen_heap_kinds,test_debug_verify,test_committed_gate_policy}.py` reference old files. Preserve their independent obligations at the shared boundary. Include `runtime/molt-cpython-abi/tests/pending_call_authority.rs`, `runtime/molt-backend/tests/{native_extern_linkage,llvm_generated_object_linkage}.rs`, `runtime/molt-jit/tests/{jit_code_owner,runner_cli,runner_launch}.rs`, `tests/cli/test_jit_run_tier.py` and `tests/molt_diff.py`. Reuse `tools/{consumer_gate,check_changed_crate_tests,generators,molt_dev}.py` and the performance authority. The production crates, generators and proof selectors for the deletion are extensions of the starting aperture. No new proof runner is required.
