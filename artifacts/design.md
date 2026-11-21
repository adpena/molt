# Binary size from closed-world reachability

- **Author** Alejandro Peña
- **Status** Proposed for Goldfish review
- **Date** 2026-11-21

## Problem and acceptance

The dispatch records a 10.6 MB macOS arm64 release executable for the 25-line `bench_class_hierarchy.py`, including 7.3 MB of `__text` and 2.6 MB of `__const`. Its first-launch scan contributes to edit-to-run latency. The V1 ledger records a 1,616,065-byte LLVM wasm32 split application against a 1 MiB budget (HF-859) and a 3.28 MiB gzip -9 shared runtime against a 3.25 MiB budget (HF-869). Those are historical observations. They establish the workloads and failure thresholds. Fresh measurements must identify the objects and roots responsible for retained bytes before a change claims savings.

Success means that the release executable and the LLVM wasm32 application carry less code and data when their reachable program is smaller. Measure the fixed startup footprint with hello and the additional bytes associated with features independently exercised by the controls. A linked WASM runtime may follow the app's closure. A reusable split runtime carries the full generated ABI for its target and capability tier, so its budget is a separate obligation even when the app shrinks. Preserve the verified subset, runtime and host ABI, C-extension link contract, launch time, and executed instruction count. Use the existing budget tests unchanged. [V1 findings ledger](../docs/agent/V1_HANDOFF_FINDINGS.md)

## Existing authorities and prior art

- Native executable links already enable section collection through `native_link_policy`, with a narrow dynamic export surface on Mach-O and ELF. The separately linked runtime image has a distinct policy. LLVM wasm32 links already request `--gc-sections`, and split app links use `--no-export-dynamic`. Compare the selected commands and real relocations before assigning the size to an archive or adding flags. [Native link command](../src/molt/cli/native_link_command.py) · [WASM object linker](../tools/wasm_object_link.py)
- Molt's source function closure and final typed native closure already share `reachable_with_callees`. `AppCallableManifest` captures address-taking for dynamic builtins, intrinsic acquisition and GPU descriptor targets. The module registry emits init relocations. The generated entry enum owns external compiled entry names. LLVM emits these metadata references before optimization. Its optimizer currently gives external function definitions temporary `DLLExport` retention, which makes external visibility an optimization decision too. [Function closure](../runtime/molt-tir/src/passes/dead_functions.rs) · [Resolver manifest](../runtime/molt-tir/src/passes/app_callable_manifest.rs) · [LLVM optimizer](../runtime/molt-backend-native/src/llvm_backend/mod.rs)
- `reached_intrinsic_symbols` and generated availability gates predict runtime features from prepared IR. The emitted app import ABI decides the final WASM runtime requirements, including extension or lowering-introduced imports. The Cargo feature graph owns tier selection. The split export helper intentionally includes all generated runtime imports, runtime-owned dynamic intrinsics, host exports and CPython ABI exports and data, plus build-specific obligations. [Runtime feature contract](../docs/design/foundation/feature_reachability_tree_shaking.md) · [WASM export planner](../src/molt/_wasm_runtime_exports.py) · [Runtime WASM build spec](../src/molt/cli/runtime_wasm_build_spec.py)
- Go's linker follows real relocations from roots and uses additional compiler-marked edges for interface and reflective calls. The transferable idea is to represent edges the linker cannot infer explicitly. LLVM internal linkage provides a local visibility boundary. `llvm.used` adds a preservation root, and wasm-ld section collection starts at the entry and exported or otherwise retained symbols. Use these mechanisms with Molt's own typed identities and ABI. [Go linker dead-code pass](https://go.dev/src/cmd/link/internal/ld/deadcode.go) · [LLVM linkage and preservation](https://llvm.org/docs/LangRef.html#linkage-types) · [wasm-ld](https://lld.llvm.org/WebAssembly.html#garbage-collection)

The lane has no committed or stashed implementation of the reported internal-linkage chip in the checked refs. The live implementation already internalizes the WASM entry wrapper's renamed body. Use that convention as a local precedent. The native final-body ownership changes visible on `origin/main` should be taken through the normal lane update before the source slice. The checked versions leave the proposed LLVM clone decision at the same boundary.

## Architecture and implementation sequence

### 1. Attribute the bytes on the existing build paths

Use one freshness-checked release build of each input per cell for structural measurement. Use `print("hello")`, `bench_class_hierarchy`, `bench_json_roundtrip`, `bench_etl_orders` and `bench_matrix_math` as the common workload set. The linked LLVM wasm32 and split modes run the same supported sources where their declared profile supports them. Also measure the existing HF-859 split application and its shared runtime. Use identical flags and inputs for baseline and candidate. Preserve the compiler, runtime, target, profile, link arguments, and artifact identity with the receipt. Label any witness the target rejects.

Capture three views from the existing boundaries:

1. Prepared IR and `reached_intrinsic_symbols` show the program's early runtime requirements. The callable manifest dump shows address-taken runtime and compiled entries. Inventory the lowering-introduced references separately from these prepared inputs. LLVM IR before and after optimization identifies emitted definitions, declarations, and relocation-producing data, including the resolver and module registry.
2. Native app and runtime archive symbols, the executable link map, and sampled `-why_live` queries identify the real linker roots for high-byte members. For LLVM wasm32 use the published app's imports and exports and map types with the generated ABI, plus the linker map or relocatable symbols for linked mode. Record launch glue, registry and callable-table references, direct app references, and the runtime's transitive references. An undefined reference in an unlinked app object can originate in a body the final linker removes. A linked runtime symbol without a direct app edge can be needed transitively. Record unattributed bytes as such.
3. `tools/binary_size_analysis.py` supplies native section and symbol attribution and WASM code metrics. Use `tools/wasm_size_audit.py` for the existing WASM section view and the split suite's content-addressed gzip -9 evidence for HF-869. Report native executable bytes and named segments/sections, WASM code/data/custom and total bytes, and the app and shared split members independently. Record the largest reachable runtime owners by evidence, including data and padding where the tools leave a residual.

Run the diagnostics in lane scratch. Use the existing diagnostic settings and inspection tools. An initial symbol join can be scratch work with `RuntimeFn`, `FunctionId`, the generated ABI, and the existing object readers as authorities. Artifact symbol strings are open to the toolchain and are joined at the file-format boundary. An absent name in a stripped binary carries no reachability claim. Where link maps or named symbols are unavailable, record section totals and the attribution gap.

### 2. Give LLVM-specialized clones object-local linkage

The first source candidate addresses functions introduced by the owned module phase. Its `owned.clones` records provide the final row identities, before declaration and before the LLVM optimization pass. A specialization clone is created inside the object after partitioning and cannot be a target in a sibling object's input. Existing function addresses inside this LLVM module are real IR references. Use `FunctionId` to select each clone's LLVM definition and mark it `Internal` before optimization. Keep the decision at `native_backend/simple_backend/compile_driver.rs`, where it already walks the typed rows to declare their functions. Keep `lowering::declare_tir_function` as the single declaration authority, applied once to each row. Plain bodies and externs at the partition boundary retain their existing linkage.

The initial A/B must isolate this choice. Attribute the change in both LLVM output bytes and final release bytes. The physical linker may already collect some of the same bodies. If the candidate has no shipped size benefit and no material build benefit, drop it from the landing and use the attributed roots to select the next change. A synthetic dead clone establishes the mechanism, while the five workloads establish its user impact. Existing rooted function retention in `module_metadata.rs` remains the authority if a later measured slice needs a terminal TIR walk shared by Cranelift and LLVM.

### 3. Prune the dominant runtime and app roots

The native decision is driven by the root attribution. Start with the existing Cargo-derived tier and archive/link plan. For a root that should be absent in a smaller program, remove the dependency at its producer, preserving demanded relocation paths. If an app-level resolver row demonstrably originates only in a dead publisher, fix the existing `AppCallableManifest` preparation and its partition consumers together. If fixed runtime support retains optional code via legitimate dynamic behavior, use the typed program fact at that support producer and retain the generic path in cells that can use it. A feature/profile change must keep publisher, fingerprint, installed-runtime and link-validation consumers aligned and justify its cold-build and artifact-cache costs. Plan for these decisions from the measured owners before choosing paths or writing a new record.

For linked WASM, use the app's emitted imports with the existing generated import-to-export and feature mapping. For a shared split runtime, preserve the full ABI and compare the retained HF-535 fixed generation with an HF-869 breaching generation if both artifacts can be recovered. Fix the largest attributable producers beneath those roots, then exercise consumers whose ABI bodies/data moved. If the retained artifacts are missing or incompatible, report the comparison as unavailable and bisect a reproducible source baseline if the current budget still fails. Do not select a per-app export subset for a shared member.

For remaining HF-859 app code, rank the live app bytes after clone linkage by function and shared lowering family. The ledger has already measured `default<Oz>` and cold code. If reached exception, literal or init operations dominate, use existing shared lowering and the owning typed facts to shrink that path. A reserved producer requires the integrator's resolution. Report independently whether the measured candidate closes HF-859, HF-869 and native size proportionality.

## Alternatives considered and risks

| Option | Consequence |
| --- | --- |
| Enable more linker collection and change LLVM's optimization level globally | Existing native and WASM links already collect dead sections. `default<Oz>` barely moved HF-859 in the retained measurement. Keep link-policy changes for measured roots whose behavior varies by linker or profile. |
| Internalize every compiled body outside the launcher entry list | Cached stdlib members, batch objects and externally callable entries can bind across objects. A name-only list misses those edges. The first slice changes compiler-created clones with object-local provenance. Broader internalization needs an authoritative cross-object consumer inventory. |
| Build a custom runtime archive per app | This could prune roots, but adds another Cargo/tool identity for each reached set and can raise cold-build latency and cache storage. First use existing tier/feature plans and linker collection. Compare cold and unchanged rebuilds before proposing a finer archive identity. |
| Remove unused exports from the shared runtime | It would make separately built apps and native extensions dependent on which app generated the shared member. Keep the generated public ABI as the shared contract and shrink its implementations and transitive dependencies. |

The correctness risk is a dropped call, relocation or data definition that is exercised through a registry, indirect dispatch, extension ABI or sibling object. Negative size evidence alone cannot detect that loss. Require independent execution on those paths, unchanged approved public exports, and explicit section counts. A first-launch wall time comparison can be confounded by signing, linker variant or page-cache state. Use fresh distinct paths for first launch, interleave the measurements, record host and signing mode, and also measure steady execution instructions. A new proof record format or instrumentation is unnecessary for the proposed first slice.

## Global invariants

1. A clone's locality is decided once from the final module pipeline's typed `FunctionId` rows. No symbol pattern, suffix rule or backend-independent copy classifies a clone. The compiler emits LLVM's `Internal` only for proven local definitions in this slice. LLVM still follows actual references inside the module.
2. Any root crossing an object or host boundary remains represented by its existing ABI entry, explicit public export, partition declaration, or real relocation. Direct links and resolver/registry metadata keep their current ABI owners. ABI catalogs and existing generator checks own their projections.
3. Source requirements, linker roots, and final bytes have distinct evidence. Each comparison records its selected source/tool/profile and artifact identity. A smaller archive or a lower root count is insufficient for a release-size claim.
4. The shared split runtime's public surface remains derived from the generated ABI and declared capability tier independent of app closure. The existing budget and ABI tests apply to the published bytes.

## Implementation skeleton

First source slice in `native_backend/simple_backend/compile_driver.rs`. This consumes the existing `TirOwnedModule` clone record and `declare_tir_function` return value inside `SimpleBackend::compile`. The helper expresses the visibility decision and can be inlined into the existing declaration loop if review prefers.

```rust
/// Apply visibility to the LLVM definition declared by the current final row.
/// Requires: `id` is the document's final FunctionId, `function` is that row's
/// owned LLVM definition, and `local_clones` is projected from `owned.clones`
/// after object partitioning. Call before LLVM optimization (invariants 1–2).
/// Ensures: identical declaration and ABI, with Internal linkage for a proven
/// clone. Raises: the existing typed-row and LLVM verification boundaries
/// reject inconsistent final rows.
fn apply_specialized_clone_linkage(
    id: molt_ir::FunctionId,
    function: inkwell::values::FunctionValue<'_>,
    local_clones: &molt_ir::function_table::FunctionSet,
) {
    todo!()
}
```

LLVM declarations cross the existing name boundary through `declare_tir_function`, with typed `FunctionId` used for locality. Extern declarations follow `declare_extern_tir_function`. Structural coverage should check that both a live local clone and an externally consumed original use the same definition path and retain their respective linkage. The existing typed producer, Rust type checking, and generated ABI checks cover the relevant authorities. The proposed code introduces no loose strings or dictionaries and uses the existing environment, transport and registry boundaries.

## Verification and landing

- The first falsifier is an exact baseline comparison of emitted LLVM objects against their final link products. If no material unused clone bytes survive, proceed directly to the largest measured source of retained bytes. Where retained runtime code dominates, inspect at least one shortest evidenced root path for each materially changed family.
- Extend `llvm_generated_object_linkage.rs` for the local/live clone and externally used body case if the current generated-object cases do not exercise it. Use the existing object inspector and link/run the witness. Exercise whole app, batched app and shared-stdlib object paths in the declared LLVM cell. Run the affected native and LLVM differentials for direct calls, address-taken callable lookup, registry/import, generators/tasks and dynamic method dispatch. Use the existing WASM linked and split host tests for startup and imports, including the C ABI consumer when a runtime link boundary changes. Compare output and exit to the uv-managed CPython oracle in isolated mode. A missing profile, toolchain or host cell remains explicitly unverified.
- Run the five before/after size and launch measurements with the existing instructions authority and separate first-launch and steady execution runs. `tests/findings/test_wasm_app_size.py` and `tests/test_wasm_split_runtime.py` own the two split budgets. Use the existing source-tree fingerprints and feature-on Rust build, then run the changed crate lib/Clippy cells through the existing consumer gate and landing precheck. Reuse exact receipts. [Instructions methodology](../tools/PERF_AUTHORITY.md)
- Record each landed slice and its impact in the V1 ledger. Drop a candidate whose evidence fails the changed obligation or revert a landed regression through normal integration. Keep the lane's measurement outputs under its scratch root and record the receipt locations and durable numbers in the lane report.

## GoldfishContext

| Boundary | Review inputs and consumers |
| --- | --- |
| Function producer, typed roots and local LLVM linkage | `runtime/molt-backend-native/src/native_backend/simple_backend/{compile_driver,program_pipeline,module_metadata}.rs`, `runtime/molt-backend-native/src/llvm_backend/{mod,lowering,app_resolver,module_registry,program_slots,wasm_entry}.rs`, `runtime/molt-tir/src/tir/pipeline_cache.rs`, `runtime/molt-passes/src/tir/module_phase.rs` (reserved, read-only), `runtime/molt-ir/src/function_table.rs`, `runtime/molt-tir/src/passes/{dead_functions,runtime_roots,app_callable_manifest}.rs`. First code changes are limited to `compile_driver.rs` and the existing linkage test if needed. |
| Cross-object producers, launchers and ABI | `runtime/molt-backend/src/backend_process/native_batch/{application/single,application/batched/jobs,worker/compile}.rs`, `runtime/molt-backend/src/backend_process/shared_stdlib_cache/{prepare/mod,compile/batched}.rs`, `src/molt/cli/native_main_stub.py`, `src/molt/cli/native_link_command.py`, `tools/wasm_object_link.py`, `runtime/runtime_abi.toml`, `runtime/molt-ir/src/runtime_callable_abi_generated.rs`, `src/molt/_wasm_abi_generated.py`. No shape change for the native worker job, partition archive or launch ABI in the first slice. |
| Requirements, existing diagnostics and binary readers | `src/molt/cli/{backend_ir,required_features,function_references,runtime_features,native_symbol_inspection,backend_diagnostics}.py`, `runtime/molt-backend-native/src/app_resolver_abi.rs`, `src/molt/environment_registry.toml`, `src/molt/backend_environment.json`, `runtime/molt-runtime/src/intrinsics/{manifest.pyi,categories.toml}`, `src/molt/_runtime_feature_gates.py`, `tools/{gen_intrinsics,gen_wasm_abi,binary_size_analysis,wasm_size_audit}.py`, `src/molt/wasm_artifact.py`, `tests/tools/test_binary_size_analysis.py`. Existing dump settings feed inspection only. |
| Runtime selection and product consumers for a later measured slice | `src/molt/_wasm_runtime_exports.py`, `src/molt/cli/{runtime_wasm_build_spec,runtime_wasm_build_support,runtime_wasm_validation,runtime_wasm_generation,runtime_fingerprints,runtime_build_identity,runtime_identity_schema,runtime_cargo_plan,installed_runtime,link_fingerprints,cache_fingerprints}.py`, `runtime/molt-runtime/Cargo.toml`, `tools/release/runtime_cells.py`, `runtime/molt-runtime/tests/wasm_cdylib_exports.rs`. Reconcile their capture, publication and validation lists if the measured runtime slice changes an input or an ABI row. |
| Selection and independent witnesses | LLVM existing `runtime/molt-backend/tests/llvm_generated_object_linkage.rs`, native control `runtime/molt-backend/tests/native_extern_linkage.rs`, LLVM sibling `runtime/molt-backend-native/src/native_backend/simple_backend/tests/llvm_backend.rs`, callable manifest fixtures `runtime/molt-tir/src/passes/tests/manifest.rs`, `tests/molt_diff.py`, `tests/findings/test_wasm_app_size.py`, `tests/test_wasm_split_runtime.py`, `tests/wasm_linked_runner.py`, `tools/proof_plan.toml`, `.github/proof-plan.generated.json`, `tools/{consumer_gate,check_changed_crate_tests,molt_dev,perf_instructions}.py`. Existing LLVM proof inputs cover the source and witness, so no new proof-plan or fingerprint role is needed for the first slice. |
| Durable design and status | `docs/design/foundation/{60_tree_shaking_whole_program_dce,61_binary_size_and_output_optimization,feature_reachability_tree_shaking}.md` own the contract and mechanism. `docs/agent/V1_HANDOFF_FINDINGS.md` owns the measured release status. This review artifact proposes code and proof only. |
