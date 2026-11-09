# Guarded calls and positional default facts (HF-773 D1'/D2')

Status: proposed; implementation waits for the integrator's Goldfish round 2.
The 2026-11-09T23:22Z correction is the settled semantic contract. D0 is an
observer only. This design does not authorize a backend-specific fast path.

## Problem and scope

Slice C decided 16 of 3,152 previously undecided branches in its published
five-program sample. Most source-level None tests concern parameters, and
CALL_GUARDED remains opaque to TIR. Passing a constant does not help a callee's
SCCP until the guarded success region makes that binding visible. The older
62% omitted-default estimate mixed methods by name; it is not the ceiling for
named function calls. D0's exact-symbol, actual-TIR census is the sizing input.

The existing method-default study measured about 0.88 s for obj.m(i) against
CPython 3.14's 0.19 s. Preserve that optimization purpose, but methods remain
HF-216's cohort: this first cut handles plain positional functions only.
The old version-zero proposal is unsound for same-code functions with other
defaults and for mutation of a keyword-default dictionary in place.

D1' makes omitted positional None/True/False arguments visible behind live
admission. D2' exposes the admitted edge to the existing inliner and reuses its
ordinary SCCP rerun. Original generic entries and CALL_GUARDED fallbacks stay.
No cloning, complete-caller inference, class/range lattice, keyword-only default
padding, varargs packing, closure promotion, or method devirtualization is added.

## Prior art and alternatives

LLVM's [CallPromotionUtils](https://llvm.org/doxygen/CallPromotionUtils_8cpp_source.html)
versions an indirect call into a guarded direct arm and the original indirect
fallback. Reuse that control-flow shape; a Python code-entry comparison needs
the additional binding/global/default custody below. [mypyc's annotation
contract](https://mypyc.readthedocs.io/en/stable/using_type_annotations.html)
checks dynamic/static boundaries. It supports using *proved* entry facts;
it does not make an annotation or a source name an exact Python identity.
In current Molt, object/ops_convert.rs::molt_guard_type counts a mismatch and
returns the original value. Check-policy GUARD_TAG is therefore a hint, not an
enforced admission proof. D0's annotation nullness solve is counterfactual;
D1'/D2' must not seed parameters from it. Any later enforced-annotation change
needs its own semantic contract and complete consumer migration.
Existing Molt precedents are the TIR inliner, its activation/drop protocol,
the runtime's compiled-body materialization admission, and HF-821's shared
fast-path lowering. Reuse them rather than creating a second optimizer.

Rejected alternatives:

- Version-zero/default-writer census: a version describes field replacement,
  not live contents. A new FunctionType may share code and have other defaults.
- Captured function-object identity alone: __code__ can change on that object;
  runtime executable/ABI/code admission is still required.
- Binding every immutable literal: separately materialized strings, floats,
  bytes and large ints can break Python object identity. Admit only singleton
  immediate words, and only for omitted positional parameters.
- Padding within each backend: duplicates policy and conflicts with HF-821.
  Ordinary TIR load/compare/branch operations are the common expression.
- Global parameter seeding: unknown/imported callers invalidate it. The
  guarded success region is the scope of each supplied/default fact.
- Specialization clones now: add module minting, identity and code-size costs
  before measuring the remainder after bounded inlining. D3/D4 stay deferred.

## Architecture and global invariants

1. **One identity boundary.** Reuse frontend CallableId(module, PythonSourceKey).
   Resolve it to the existing emitted symbol only at transport/link boundaries.
   The module assigns dense FunctionId values once; CallGraph owns no competing
   name identity. Source names, runtime NameId and source CallableId are distinct.
2. **Live admission.** The existing CALL_GUARDED executable/ABI/code check must
   succeed first. Admission also preserves globals/code-slot custody and every
   invocation duty the shared call path requires. Then each omitted positional
   default is read from the *current* defaults tuple and compared to its expected
   singleton word. No version, name hint, AST writer census or fresh-object stamp
   substitutes for either check. Missing/short/non-tuple defaults select slow.
3. **One live-default reader.** Move/generalize the live tuple-reading mechanism in
   intrinsics/registry.rs::function_default_bits_are, currently shared by
   compiled-body materialization and intrinsic defaults, into the canonical
   object/function_metadata.rs owner and migrate both consumers. Select omitted
   parameters relative to the live tuple's tail; do not assume its length equals
   the number omitted. Already supplied parameters need no expected-default
   check. Preserve callbacks, GIL/execution-context and borrow lifetimes.
4. **Guard-local facts.** Expected singleton values exist as call argument facts
   only in the successful region. No callback, mutation, poll or safepoint that
   can replace defaults may occur between live admission and argument capture.
   Reads are memory-sensitive; no LICM/GVN may reuse them across a possible write.
5. **Exact fallback and effects.** The slow arm is the original
   CALL_GUARDED(callee, supplied...), with its source location, result, exception
   continuation and custody intact. CALL_BIND is not a substitute. The opaque
   fallback remains in call effects, recursion, reachability and IP summaries.
6. **Existing inline safety.** Retain execution_context, physical partition,
   non_inlinable shared stdlib, recursion, handlers, generators, entry-predecessor,
   closure-env index, argument incref, parameter-custody and budget gates.
   A new GuardedDirect edge does not waive them. Unsupported cells stay generic.
7. **One lowering.** HF-821 owns shared callable admission and its ordinary TIR
   lowering on native, LLVM and WASM. D1'/D2' consume that owner; no code in the
   reserved backend lowering directories is changed by this lane. If that
   owner cannot express the plan, return the dependency to the integrator.
8. **Indivisible replacement.** Replace func_default_specs and its projections
   with one FunctionBindingSpec/DefaultSpec boundary, including sema, cache,
   workers, protocol declarations and test fakes. No dict adapter or legacy
   registry survives. Retire FUNCTION_DEFAULTS_VERSION with its last producer/
   consumer only through its owning HF-821 integration, including generated
   tables and backend consumers; do not delete a reserved lowering in this lane.

### D1': typed binding and live defaults

FunctionBindingSpec is keyed by the existing CallableId. ParameterKind is a
closed enum; ParamIndex denotes a position in the declared ABI, including any
environment parameter. The frontend explicitly maps source arguments to those
positions. DefaultSpec distinguishes no default, singleton, other literal and
dynamic expression by typed variants. Open source identifiers are read from
their source keys at the keyword/metadata boundary, never used as fact keys.
The full schema preserves other existing binding consumers; first-cut promotion
admits no keyword-only parameters, *args/**kwargs, closures or decorated entries
whose runtime candidate cannot satisfy the shared admission contract.

After HF-821 admission, emit live loads, singleton comparisons and a conditional
branch in TIR. The successful arm calls the original compiled entry with supplied
arguments plus admitted singleton values. The slow arm keeps the original op.
Runtime invocation/global publication must either be preserved by shared lowering
or be discharged by its existing typed proof; a raw CALL cannot silently drop it.
The function's generic public entry is never rewritten with call-site facts.

### D2': guarded edges and existing inlining

CallEdge::GuardedDirect carries FunctionId and a typed guard-region proof; it
describes the success edge, alongside the original opaque fallback. The graph
and CallFacts keep those roles distinct. The inliner visits the success region,
applies its one eligibility authority and activation protocol, then re-runs the
ordinary per-function pipeline over the spliced caller. Extend the benefit
trigger to a proved singleton/nullness branch opportunity; do not loosen safety
or invent another cost unit. No new function is minted, so module idx_map and
FunctionIR back-conversion remain one-to-one. Cloning requires a later design.

## Binding skeleton (illustrative, not production stubs)

```python
class SingletonDefault(Enum):
    NONE = 0
    TRUE = 1
    FALSE = 2

class ParameterKind(Enum):
    POSITIONAL_ONLY = 0
    POSITIONAL_OR_KEYWORD = 1
    KEYWORD_ONLY = 2
    VARARGS = 3
    VARKW = 4
    CLOSURE_ENV = 5

@dataclass(frozen=True, slots=True)
class NoDefault:
    pass

@dataclass(frozen=True, slots=True)
class SingletonSpec:
    value: SingletonDefault

@dataclass(frozen=True, slots=True)
class OtherLiteralSpec:
    # Literal data are open values, not names or fact keys.
    value: int | float | complex | str | bytes | EllipsisType

@dataclass(frozen=True, slots=True)
class DynamicSpec:
    expression: PythonSourceKey

DefaultSpec = NoDefault | SingletonSpec | OtherLiteralSpec | DynamicSpec

@dataclass(frozen=True, slots=True)
class ParameterSpec:
    index: ParamIndex
    source: PythonSourceKey | None        # None only for the implicit env
    kind: ParameterKind
    default: DefaultSpec

@dataclass(frozen=True, slots=True)
class FunctionBindingSpec:
    callable: CallableId                 # reuse the existing source identity
    parameters: tuple[ParameterSpec, ...]
    kind: FunctionKind

@dataclass(frozen=True, slots=True)
class DefaultPadPlan:
    target: CallableId
    supplied: int
    omitted: tuple[tuple[ParamIndex, SingletonDefault], ...]

def default_pad_plan(spec: FunctionBindingSpec, supplied: int) -> DefaultPadPlan | None:
    """Requires a complete source-to-ABI map (I1, I8). Ensures only the
    positional singleton cohort is proposed; no identity/admission is proved
    here (I2-I4). Returns None for a missing required parameter or excluded
    shape; malformed source metadata raises the existing frontend diagnostic.
    """
    ...
```

```rust
// Leaf IR owns ParamIndex (migrate the D0 observer's local index, no duplicate).
pub struct ParamIndex(u32);
pub struct FunctionId(u32);                 // module-owned dense index
pub struct GuardRegionId(u32);              // shared admission's success region
pub enum SingletonDefault { None, True, False }
pub struct OmittedDefault { parameter: ParamIndex, expected: SingletonDefault }
pub struct DefaultPadPlan { callee: FunctionId, omitted: Box<[OmittedDefault]> }
pub enum CallEdge {
    StaticDirect(FunctionId),
    GuardedDirect { callee: FunctionId, success: GuardRegionId },
    Opaque,
}

/// Requires shared executable/ABI/code/global admission and a live callee
/// borrow (I2, I3, I7). Ensures a typed, memory-sensitive defaults load/compare/
/// branch plan with no intervening callback; false goes to the original op
/// (I4, I5). Invalid/excluded plans return the existing typed refusal.
fn lower_default_admission(
    caller: &mut TirFunction,
    site: CallSiteId,
    plan: &DefaultPadPlan,
    admission: &SharedCallableAdmission,
) -> Result<GuardRegionId, PromotionRefusal> { todo!() }

/// Requires a module-owned function table and typed successful region (I1).
/// Ensures inlining uses the existing eligibility/activation/splice owners
/// and reoptimizes only successful bodies; fallback effects persist (I4-I7).
/// Refused sites are unchanged; malformed proof/SSA is a verifier error.
fn inline_guarded_site(
    module: &mut TirModule,
    site: GuardedCallSite,
    facts: &ModuleCallFacts,
    tti: &TargetInfo,
) -> Result<InlineOutcome, VerifyError> { todo!() }
```

## Qualification and risks

For each D1'/D2' slice: measure instructions retired over the entire HF-572
table, explain every head/base ratio above 1, and retain compile time, peak RSS,
startup and artifact/text size. Keep identical workloads, actual tool/image/
source identities and frontend policy. The multiprocessing counter limitation
on Regex Redux must be resolved or explicitly remain unknown; parent-only
instructions do not qualify the whole workload. Use the D0 static census to
bound opportunities; it is not dynamic execution frequency or speedup evidence.

Extend the existing CPython differentials module_func_defaults_mutation.py,
direct_default_metadata_reassignment.py and call_argument_expansion_custody.py:
positional __defaults__ replacement before/inside a loop; in-place
__kwdefaults__ mutation (excluded cohort stays generic); FunctionType sharing
code with different defaults; alias/rebind and __code__ replacement; a closure;
None/True/False identity and value; other immutable and mutable defaults;
too few/many arguments and exact error behavior; globals/code-slot custody;
exception/traceback and parameter ownership on both arms. Run native, LLVM and
WASM under their declared profiles and exact versioned CPython authorities.

TIR regressions independently check branch/SSA validity and output semantics,
the stale candidate's fallback, live-default tail alignment, callback barriers,
unchanged generic entries, non-inlinable stdlib and execution-context/partition
refusals. Use the existing verifiers and inliner safety tests. Structural review
and generated checks enforce I1/I7/I8; output tests cannot prove one authority.
No permanent gate, cache, pass runner or profiling registry is introduced.

The primary risks are duplicated admission, a read hoisted past mutation,
dropping invocation/frame ownership, incomplete metadata consumers and code
growth. I2-I8 address soundness; whole-table measurement decides benefit.
Potential opportunity eliminated by the existing execution-context or partition
gates must be reported as excluded, never presented as an achieved gain.

## GoldfishContext: complete consumer migration aperture

Paths below are relative to the checkout. These are review inputs and future
implementation owners, not a claim that D0 changed them.

- Frontend identity/default producers: src/molt/frontend/_types.py,
  lowering/function_metadata.py, lowering/symbol_naming.py,
  lowering/generator_state.py, lowering/local_bindings.py,
  lowering/sema_state.py, sema/funcmeta.py, sema/result.py, sema/__init__.py,
  visitors/functions.py, visitors/async_gen.py,
  visitors/class_method_compilation.py, visitors/statement_scope.py.
- Frontend readers/protocols: src/molt/frontend/_protocol.py,
  _protocol_attrs.py, visitors/call_dispatch_named.py,
  visitors/call_module_dispatch.py, visitors/call_runtime_helpers.py,
  lowering/serialization_basic_ops.py; src/molt/cli/models.py,
  frontend_pipeline.py, frontend_execution.py, frontend_worker.py, module_cache.py.
  All five CALL_GUARDED producer sites must be audited. D1' initially changes
  the two plain named-function sites; closure/module/runtime-helper sites keep
  their existing behavior unless their typed admission is separately proved.
- Frontend boundary fakes/readers: tests/test_frontend_builtin_call_lowering.py,
  tests/test_frontend_sema.py; tests/cli/test_cli_module_cache_authority.py,
  test_cli_frontend_partial_progress.py, test_cli_frontend_scc_scheduling.py,
  test_cli_lowering_cache_tooling_scope.py,
  test_cli_lowering_context_ast_idempotent.py,
  test_cli_module_frontend_shared_cache.py, test_cli_import_collection.py,
  test_cli_lowering_context_encoder_fixed_point.py,
  test_cli_frontend_worker_authority.py,
  test_module_analysis_scan_admission.py, test_cli_typecheck_authority.py.
- Module/call consumers: runtime/molt-passes/src/tir/{call_graph.rs,
  call_sites.rs, call_facts.rs, call_facts/site_analysis.rs, fact_graph.rs,
  module_phase.rs, mod.rs}; passes/ip_summary.rs;
  passes/inliner/{driver.rs,eligibility.rs,call_sites.rs,splice.rs,activation.rs};
  passes/generator_fusion/{recognize.rs,driver.rs}; call_sites/tests.rs,
  call_facts/tests.rs, passes/inliner/tests, passes/generator_fusion/tests.rs;
  runtime/molt-passes/tests/tir/ownership_memory/drop_insertion/call_custody.rs;
  runtime/molt-backend/src/fact_graph_emit.rs;
  runtime/molt-backend-native/src/native_backend/simple_backend and its tests;
  runtime/molt-tir/src/tir/{mod.rs,pipeline_cache.rs,module_metadata.rs}.
- Vocabulary/projections: runtime/molt-ir/src/tir/{function.rs,ops.rs,
  op_kinds.toml,serialize.rs,verify.rs}, runtime/molt-ir/src/ir.rs and ir_schema.rs;
  tools/gen_op_kinds.py and its generated Python/Rust projections. Update all
  transport/cache contract hashes when binding/guard facts enter their schema.
  No stringly guard proof is hidden in AttrDict.
- Runtime semantic owners: runtime/molt-runtime/src/intrinsics/registry.rs,
  object/function_metadata.rs, object/layout.rs, call/function.rs,
  builtins/functions/function_abi.rs, intrinsics/manifest.pyi. Reuse the live
  tuple reader and canonical NameId metadata fields. A needed generated runtime
  ABI change is integrated by its reserved-path owner, not edited here.
- Proof/document authorities: tools/proof_plan.toml (existing src/runtime globs
  already select these families), tools/check_changed_crate_tests.py,
  tools/molt_dev.py land --precheck-only, docs/OPERATIONS.md,
  docs/design/foundation/call_anatomy.md,
  docs/spec/areas/compat/contracts/call_argument_binding_contract.md and the
  existing V1 ledger. No new environment variable is proposed. D0 reads only
  the existing TIR_OPT_STATS/MOLT_EMIT_PASS_DELTA switches and writes under the
  existing MOLT_DEBUG_ARTIFACT_DIR boundary; pass_delta_dashboard.py and
  translation_validator.py keep their existing stream/schema.

Typed-fact audit: the proposed optimizer records have no str/dict/Any fact
payload. CallableId is reused, not redefined; its open module-name component
stays at the existing source identity boundary. PythonSourceKey identifies
source expressions; ParamIndex identifies ABI positions; FunctionId identifies
module functions; GuardRegionId identifies dominated success regions. JSON
keys, source spelling and emitted symbols are boundary encodings only. D0's
CandidateSymbol is deliberately diagnostic and never a callable proof.
