# Native backend early sealing

Status: accepted design; implementation qualification pending. This describes the native backend's intended sealing
invariant and its validation, not implemented or qualified behavior. HF-807 in the
[V1 ledger](../agent/V1_HANDOFF_FINDINGS.md) owns current implementation and qualification status.

## Problem and evidence

HF-807 records that compiling `asyncio_events__molt_module_chunk_6` (7,402 SimpleIR operations)
peaks at about 3.25 GB RSS and failed a cold asyncio build with an attempted 3,288,334,352-byte
allocation. The supplied Massif evidence attributes growth to `seal_all_blocks` -> Cranelift
`SSABuilder::use_var_nonlocal` -> `find_var`, reaching about 143 million DFG values. Those numbers
come from the existing V1 ledger; matching replay evidence is required before qualification. A quiet A/B
in that ledger found the cost both with and without HF-773, so this is not evidence against the
parallel mid-end migration.

The inspected native function compiler declares a Cranelift `Variable` for each SimpleIR name and
initializes its entry definition. Handler live-ins already use explicit `BlockTransportPlan`
parameters. They must be preserved. The existing direct-SSA analysis and exception transport
remain their respective authorities.

Inspection confirms these deferred-sealing paths:

| Block family | Current producer and late-seal mechanism |
| --- | --- |
| Textual and handler labels | `function_compiler/block_control.rs::materialize_label_block` and `fc/ret_jump.rs`; materialization does not seal labels, including forward labels and later backedges. |
| Structured if merge | `fc/control_flow.rs`; the merge at `EndIf` is left unsealed whenever the function has any exception labels. |
| Loop exit and some loop continuations | `fc/loops.rs`; several seal sites are conditional on the same function-wide exception-label test. |
| Unrolled reduction headers | `fc/loops.rs`; both the unrolled header and scalar epilogue header rely on the final sweep after their respective backedges. |
| State dispatch fallback and resume targets | `fc/coroutine.rs::StateSwitch`; `Switch::emit` creates additional internal blocks; fallback and resume blocks require explicit accounting. |
| Return | `function_compiler.rs`; the common return is already sealed after its incoming branches. Keep that ordering. |

Internal arithmetic diamonds and exception-check fallthrough blocks mostly already seal promptly.
The change must preserve these fast paths rather than wrap every instruction in a second CFG.

## Goals and boundaries

Seal each native block as soon as its complete incoming set is emitted. Remove the production
`seal_all_blocks` sweep and fail codegen on a missing seal, extra predecessor, or outstanding
incoming obligation. Preserve Python behavior, explicit block arguments, cleanup ownership,
generated-code quality, and CLIF verification.

First measure which block/variable families create parameters; then implement sealing; then
compare replay and cold aio costs and execute the required proof cells. Do not expand
`NativeSsaValues`, move all temporaries to memory, change Python policy, introduce a runner or
environment setting, or modify the frontend or mid-end for this native-only mechanism.
The ledger owner records resulting qualification; this design does not duplicate burndown state.

## Prior art and reuse

Cranelift's pinned `cranelift-frontend` 0.137.1 source is the API authority, not a different online
version. `frontend.rs::seal_block` says to call it as soon as the last branch to the block exists;
`ssa.rs::find_var` allocates a block parameter when it cannot find an existing definition.
An unsealed block defers predecessor lookup. Prompt sealing allows its single-predecessor shortcut
to follow definitions instead. The upstream [SSA implementation](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/frontend/src/ssa.rs)
documents the same algorithm. This design adopts that contract; it does not copy upstream code.

Reuse `preanalyze_function_ir`'s existing `CFG::build` result, `SimpleCfgLivenessFacts`' operation
mapping, structural if/loop maps, `BlockTransportPlan`, and `seal_block_once`. In this checkout the
CFG source is `runtime/molt-ir/src/tir/cfg.rs`, not the backend crate. Its `predecessors` contains
ordinary edges; `exception_edges` and `state_resume_edges` are separate relations. A count of
`predecessors[target].len()` alone would miss executable incoming paths.

`SimpleDefinitionFacts`, execution-point dominance, and `NativeSsaValues::can_bind_directly`
are the existing authority if SSA-locals work is still necessary.
That is a later decision using sealing measurements, not another analysis in this change.

## Mechanism

### Incoming obligations and sealing

Extend the existing native preanalysis with one incoming plan derived from the shared CFG's
ordinary, exception, and resume relations and existing structured-control maps. Index records by
typed CFG/block identities in flat arrays. An incoming obligation identifies an authored source
transfer and destination, including transfer role when one source has several continuations.
Use generated `OpKind`/exception-edge identities, never source-name tests. `TryStart` registration
metadata must not count as a native branch: native emission treats it as metadata.

Project these obligations onto actual Cranelift label and structured-merge blocks as they are
allocated. Use one typed lifecycle per native block: planned incoming count, remaining count,
materialization state, and sealed state. Replace the current seal record with that authority;
do not retain an independently writable set beside it. A seal is recorded only after the actual
Cranelift call succeeds.

Emit transport arguments before registering the edge, emit the actual jump/branch, then consume
its planned incoming obligation. Reaching zero seals a materialized target immediately. If the
last incoming branch precedes textual materialization, retain zero and seal when that block is
materialized, before its first variable read. A later incoming edge to a sealed target fails.
Two arms of one CLIF `brif` targeting the same block count as one predecessor instruction, as
Cranelift does. A branch table likewise deduplicates each instruction's destination blocks.

Keep actual lowering edges distinct from the semantic CFG relation. Exception `Origin` creates a
cold intermediate block whereas `Unwind` branches directly; both fulfill the same authored handler
obligation only after their final branch is emitted. Internal diamonds declare their known local
budgets at creation. Loops declare entry, continue/backedge and exit obligations; headers seal
after their final emitted continue/backedge, including unroll and epilogue loops. Exits seal after all
break and ordinary exit branches. Remove function-wide `exception_label_ids.is_empty()` as a
sealing criterion; only the actual incoming plan decides completeness.

Lowering can omit a planned edge when a terminal arm or a proven lowering transformation removes
that transfer. The responsible transformation must explicitly discharge that exact obligation as
not emitted when it makes the decision; it cannot quietly decrement an aggregate at function end.
This includes skipped if bodies, metadata-only/linearized loops, specialized unrolled reductions,
and detached lowering after a filled block. An ordinary branch cannot satisfy a missing exception
or resume obligation. Mismatches are errors, not permission to seal later.

`Switch::emit` is an important boundary: one semantic dispatch can become multiple CLIF branch
instructions. Enumerate its emitted destination instructions once after emission and account for
their physical predecessors before discharging the semantic resume obligations. Preserve the
pinned helper's already-sealed private blocks; identify its new block-ID range rather than
reimplementing sparse-switch lowering. Internal refcount emission also calls `builder.seal_block`
directly. Bring its successful seals into the same native lifecycle when used inside a function.
Task trampolines use the same native builder and checked completion boundary; their errors flow
through function-object lowering. Standalone JIT probes retain their explicit local seals.

At function completion, assert that every materialized native block has zero remaining obligations
and a successful seal record. Check all native-owned allocated IDs; the pinned switch helper's
internal IDs remain helper-owned. Any allocated-but-unused block must be proven pristine, outside layout, without
incoming edges or live parameters; allocate specialized loop blocks only when needed where this
avoids such remnants. No cleanup loop may seal unknown blocks to make the assertion pass.
Cranelift's own finalize assertion is debug-only; the Molt missing-seal check must also run in the
shipping compiler profile. Keep the existing verifier and unreachable-CFG retirement boundary.

### Measurement and settled helper boundary

The integrator's 2026-11-07 correction drops the patched-Cranelift attribution experiment.
Use uninstrumented builds and report peak RSS, compile seconds and `func.dfg.num_values()` after
codegen, selected by the existing `MOLT_DEBUG_SEAL` setting. No dependency fork, diagnostic
accessor, per-class counter or new environment key is introduced.

The completion check covers native-owned blocks in every compiler profile and returns a codegen
error naming the function and block. A successful `Switch::emit` call records helper completion;
its private blocks remain helper-owned and are covered by Cranelift's own debug finalize assertion.
The native check does not query private Cranelift state.

The implementation keeps the lifecycle on the native builder so direct refcount seals participate
without another threaded seal registry. A CFG source can lower to multiple physical predecessors;
its incoming obligation is consumed once on emission. An unconsumed source closes after lowering
has passed its complete operation range, including textual fallthrough materialization. This
handles eliminated terminal arms without a final unqualified sealing sweep. Synthetic private
merges and loops close at their structural final edge. The actual CFG remains the authority.

## Alternatives and risks

| Alternative | Tradeoff and disposition |
| --- | --- |
| Keep a final lazy seal sweep | It hides incomplete emission accounting and is the measured allocation stack. Rejected for this contract. |
| Remove exception guards and seal at textual region ends | Smaller edit, but backward handler/resume edges and delayed branches can arrive later. Rejected without complete predecessor evidence. |
| Put all temps in stack slots | Avoids some SSA repair but adds generated loads/stores and loses register opportunities. Rejected as an unmeasured regression and outside the assigned first step. |
| Extend SSA-local direct binding first | Potentially useful; existing analysis already exists. Deferred until early-sealing A/B establishes residual need. |
| Build another native CFG or instrument every instruction | Duplicates semantic authority or adds repeated work. Use the existing CFG and explicit incoming obligations instead. |
| Count retained block parameters | Misses aliased placeholders; use the DFG allocation total for the settled before/after experiment. |

The main correctness risk is sealing too soon after confusing semantic and physical edges. Typed
obligation identities, branch-instruction deduplication, exception/resume regressions, and the CLIF
verifier address it. Overcounting leaves a block open and must fail rather than reintroduce the
sweep. Late reads can still create parameters even in sealed multi-predecessor blocks; early
sealing is not a claim of linear SSA construction for every function. The remaining many-handler
cost may justify later direct SSA work. The measurement reads the existing DFG allocation total without retaining per-Value events. No emitted-program ABI, runtime behavior,
external service, credential handling, or new security boundary changes.

## Implementation and proof plan

1. Measure the original chunk before changing sealing, using the settled RSS, DFG-value and compile-time observations. Use the existing batch replay boundary and exact
   manifest environment; a manifest `null` means unset. Preserve the original replay input.
2. Update `function_compiler/preanalysis.rs`, `block_control.rs`, `function_compiler.rs`,
   `simple_backend/variables.rs`, `simple_backend/refcount.rs`, `function_compiler/shared.rs`, and
   `fc/{control_flow,exception_control,ret_jump,loops,coroutine}.rs` for plan construction,
   lifecycle ownership, emitted edges, successful seals, and the completion assertion. Migrate
   internal arithmetic/helper callers if their seal-record type changes; list the full family
   below. Remove replaced state and stale comments in the same implementation.
3. Extend existing `function_compiler/tests/block_control.rs` with forward/backward label,
   re-entrant entry, duplicate-target branch, no-else/terminal-arm merge, nested loop break/continue,
   exception-origin/unwind, sparse/dense resume switch, and unroll/epilogue witnesses as needed by
   the changed obligations. Use independently verified CLIF execution/verification, not a mirror
   of the decrement algorithm. Add seeded missing-seal/edge tests that fail the production
   completion check before finalization. Update existing fixtures in
   `function_compiler/tests/{shared,cleanup_roots,scalar_carriers}.rs` when their real boundary
   changes; do not let a fake bypass it.
4. Add `tests/differential/basic/native_early_seal_joins_exceptions.py`: CPython-oracle output for
   both arms of repeated joins, loops with break/continue and handler/finally exits, including
   failure paths. Reuse the basic differential selector instead of creating a new proof cell.
5. Use an owned worktree on the assigned compiler measurement host. Scratch targets,
   temporary sources, and outputs remain under the lane's `TMPDIR`. Preflight exact Python 3.12,
   Rust/Cargo, Linux target, replay files, and verifier configuration before costly commands.
   Keep heavy compiler builds and replay off the landing host.
6. Run the owning `backend-native` plan cell: whole library/bin tests for
   `molt-backend-native`, `molt-backend`, and the selected reverse-dependency closure with
   `--lib --no-fail-fast` in its declared release-fast/features configuration. Run the assigned
   native basic + stdlib differentials against the exact CPython 3.12 oracle. Inspect matching
   existing evidence first. Shared semantic inputs are unchanged, so no new WASM implementation
   claim is made; execute any sibling cell actually selected by the owning plan. No new generator
   or runtime-op registry is introduced.
7. Repeat the uninstrumented chunk replay using the same input/toolchain/configuration with
   `/usr/bin/time -v`; report peak RSS, wall seconds and actual DFG values, with compile and whole
   process timing distinguished. Repeat a cold aio build with fresh owned artifact roots on the
   same host. Report exact source/binary/environment identities and whether SSA-locals is still
   needed. Native/WASM/LLVM, OS, architecture, profile and Python cells without execution remain
   unverified. Generated-code execution must retain output and expose any code-size/time tradeoff.
8. Run the required landing static/precheck once in the runner's plain environment with the
   selected managed interpreter and data-volume `TMPDIR`, unsetting lane target/environment overrides.
   Review the exact owned diff, commit with exact paths, and return the report. The integrator
   performs post-implementation drift review, landing, ledger reconciliation, and retirement.

The before/after/class tables belong in private evidence. Historical numbers are not a substitute
for matching baseline receipts. This proposal establishes no new measurement or proof result.

## GoldfishContext

Read the files below against this design. The integrator owns fresh-context Goldfish review;
implementation starts after settled findings return to its owner.

- Canonical semantic CFG and facts: `runtime/molt-ir/src/tir/{cfg,cfg_liveness,dominators,simple_def_use}.rs`.
- Native orchestration and allocation: `runtime/molt-backend-native/src/native_backend/function_compiler.rs`;
  `function_compiler/{preanalysis,block_control,cleanup_roots,shared,scalar_carriers}.rs`.
- Edge owners: `function_compiler/fc/{control_flow,exception_control,ret_jump,loops,coroutine}.rs`.
- Seal-record consumers, even when a site's semantics stay unchanged:
  `function_compiler/fc/{arith,arith_division,bitwise_shift,callargs,calls,compare,const_literals,funcobj,memory,subscript_get,truthiness,unary_logic}.rs`;
  `simple_backend/{variables,value_encoding,refcount,app_resolver}.rs`.
- Separate helper ownership: `simple_backend/{trampolines,jit}.rs`; pinned Cranelift 0.137.1
  `src/{frontend,ssa,switch}.rs`. Refcount and Switch direct seals cannot disappear from the review.
- Real-boundary fixtures: `function_compiler/tests.rs` and
  `function_compiler/tests/{block_control,shared,cleanup_roots,scalar_carriers,preanalysis_classification,list_index_fast_path,literal_ownership}.rs`;
  `simple_backend/tests/{codegen_regressions,field_access}.rs`.
- Existing execution oracle: `runtime/test_support/native_object_execution.rs`;
  `tests/differential/basic/{exception_control_flow,try_finally_nested_control,midend_resume_exception_transport}.py`;
  `tests/differential/stdlib/asyncio_loop_exception_handler_debug.py`; new basic regression listed above.
- Proof selection and closure: `tools/proof_plan.toml` (`backend-native` already covers
  `native_backend/**/*.rs`), `tools/{proof_plan,gen_proof_plan,check_changed_crate_tests,molt_dev,molt_dev_precheck}.py`.
  `src/molt/debug/verify.py::_read_production_source_tree` recursively reads the native source
  family and needs no new leaf registration for an in-tree module.
  No hand-maintained source list or generated projection needs a new leaf for these edits; verify
  that claim again against the final path set before expensive qualification. New Python/Rust
  helper modules outside those globs would require reopening the consumer inventory.
- Environment authority: `src/molt/{environment_registry.toml,backend_environment.json,_environment_registry.py}`
  and `runtime/molt-ir/src/backend_environment.rs`; reuse `MOLT_DEBUG_SEAL`, add no key or reader family.
- Public semantics/structure: `docs/spec/areas/testing/0007-testing.md`,
  `docs/spec/areas/compat/contracts/verified_subset_contract.md`, `docs/ROOT_LAYOUT.md`, and
  `docs/design/foundation/typed_identity_state_machines.md`.
- Scope/status context: `docs/agent/V1_HANDOFF_FINDINGS.md` HF-807. Private evidence records
  receipts; it does not become a second V1 ledger.
