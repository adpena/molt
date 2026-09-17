# Call Argument Binding: Keyword Args, `*args`, `**kwargs`

**Spec ID:** 0016
**Status:** Draft
**Last updated:** 2026-09-17
**Audience:** compiler engineers, runtime engineers
**Goal:** Add Python-compatible call argument binding (positional, keyword, varargs, varkw) while preserving Molt Tier 0 performance via specialization and allocation-free fast paths.

---

## 1. Motivation

Molt’s stated direction is “a verified per-application subset of Python” compiled AOT into small, fast native and WASM binaries. Both targets share the binding contract. Python’s ergonomic calling conventions—keyword arguments plus `*args` and `**kwargs`—are a major part of that surface, especially for APIs that rely on keyword-only parameters.

However, naïvely implementing argument binding by always allocating intermediate `tuple`/`dict` objects (or always falling back to a generic slow path) would conflict with Molt’s Tier 0 goals (structification, monomorphic call sites, and Cranelift-compiled codegen).

This spec defines:
- The **semantics** Molt must match (or deliberately restrict) for argument binding.
- A **tiered lowering strategy**: allocation-free fast paths when call shapes are statically known, and a correct generic binder when they are not.
- The required **IR and runtime primitives**.

---

## 2. Scope

### 2.1 In scope (Phase 1)
- Call sites:
  - Positional arguments: `f(a, b)`
  - Keyword arguments: `f(a=1, b=2)`
  - Star expansion: `f(*xs)` and mixtures like `f(1, *xs, 2)`
  - Double-star expansion: `f(**m)` and mixtures like `f(a=1, **m)`
  - Combined: `f(*xs, **m, a=1)` (preserve left-to-right evaluation)
  - Ordinary class calls: `T(...)`, including `__new__`/`__init__` routing
    through `type.__call__`
- Function signatures:
  - Positional-or-keyword params, defaults
  - Keyword-only params
  - Varargs param: `*args`
  - Varkw param: `**kwargs`
  - Positional-only params (`/`) **supported for binding** (parsing/AST support permitting)
- Correct `TypeError` cases for:
  - Too many positional args
  - Missing required args
  - Unexpected keyword
  - Multiple values for the same parameter
  - Non-string keys in `**kwargs`
  - Passing positional-only params as keywords

### 2.2 Not in scope (initially)
- Exact CPython `TypeError` message text parity (we target message-class + key name inclusion; exact wording can be a later tightening gate).
- Full introspection parity (`inspect.signature`, `__text_signature__`, etc.).
- Exotic mapping/iterable behaviors beyond the verified subset (see Tier rules).
- `functools.partial`-style signature rewriting.
- Binding semantics for dynamically constructed Python callables with non-Molt signatures (falls back to Tier 1 generic call, or is rejected under closed-world rules).

---

## 3. Terminology

- **Parameter kinds** (Python terminology):
  - **pos-only**: positional-only (before `/`)
  - **pos-or-kw**: positional-or-keyword
  - **kw-only**: keyword-only (after `*` or after varargs)
  - **varargs**: `*args` parameter (tuple)
  - **varkw**: `**kwargs` parameter (dict)
- **Call shape**: a compact description of a call site:
  - `npos` = number of positional arguments after star-expansion
  - `kwnames` = ordered list of keyword names (string IDs) supplied explicitly (excluding `**` expansions that may be dynamic)
  - `has_star` / `has_dstar` flags
- **Binder**: the algorithm that maps passed args into callee locals and builds `*args`/`**kwargs` objects when needed.
- **Tier 0**: fully compiled, specialization-first, minimal allocation.
- **Tier 1**: generic / guarded / runtime-assisted correctness path.

---

## 4. Semantic Requirements

### 4.1 Evaluation order

Molt must preserve Python’s evaluation order for:
1. The callee expression (`f` in `f(...)`)
2. All positional arguments (including `*expr`) left-to-right
3. All keyword arguments (including `**expr`) left-to-right

Side effects from argument expressions must occur before binding errors are raised (except for errors that can be proven at compile time without evaluating expressions).

### 4.2 `*args` expansion semantics
- `*expr` must evaluate to an **iterable**.
- Its items are appended to the positional argument list in iteration order.
- Tier rules may restrict supported iterables for Tier 0 (e.g., tuple/list/range); Tier 1 may implement a broader subset.

### 4.3 `**kwargs` expansion semantics
- `**expr` must evaluate to a **mapping** (Phase 1: dict-like mapping; see Tier rules).
- Iteration order for insertion into `**kwargs` (when collecting extra keywords) follows mapping iteration order.
- Keys **must be `str`**; otherwise `TypeError`.

### 4.4 Keyword argument semantics
- Explicit keywords: `f(x=1)`
- Keyword names match parameters by name, except:
  - pos-only parameters cannot be supplied via keyword
- Duplicates are errors:
  - Explicit keyword duplicates another explicit keyword
  - Explicit keyword duplicates a key from `**mapping`
  - Any keyword duplicates a parameter already assigned positionally
- If the function has `**kwargs`, unmatched keywords are inserted into that dict; otherwise `TypeError`.

### 4.5 Varargs/varkw locals
- If the callee signature includes `*args`, it receives a tuple of **extra** positional args (those not bound to named positional params).
- If the callee signature includes `**kwargs`, it receives a dict of **extra** keyword args (those not bound to named params).
- If `*args` / `**kwargs` are absent from the signature, passing extra positional / keyword args is an error.

### 4.6 Ordinary class constructor routing

Ordinary class calls use runtime `type.__call__` as the semantic authority
unless closed-world class analysis proves the MRO resolves `__new__` to default
`object.__new__`.

Required behavior:
- Custom, inherited, builtin, dynamic, or otherwise opaque `__new__`
  resolution must stay on the runtime class-call path. The frontend must not
  lower those calls to static `object_new_bound` allocation or inlined
  `__init__` construction.
- Default `object.__new__` plus default `object.__init__` rejects user
  constructor arguments.
- Custom `__new__` plus default `object.__init__` skips `__init__`.
- Custom `__init__` receives the original constructor arguments even when
  `__new__` is custom.
- If `__new__` returns an object that is not an instance of the called class,
  `__init__` is not run.

The static constructor-allocation fold is a Tier 0 optimization, not a
semantic fallback. Its eligibility predicate must be one-way conservative: when
class analysis cannot prove default `object.__new__`, lower through the same
runtime class-call/binder machinery as dynamic calls.

---

## 5. Tiered Implementation Strategy

### Shared builtin and class-hook authority

`call/bind/builtin_args.rs::builtin_call_binding` selects specialized builtin
argument handling. Raw vector calls, trampoline admission, method inline-cache
plans, and the binder use that same selection. Trampoline availability is not
evidence that Python arguments already match the runtime ABI. Already-bound
execution uses the explicit bound-call lane and must not recursively rebind.

Builtin `object.__init_subclass__` is a classmethod descriptor. Direct class,
inherited class, instance, and class-mode `super` lookups all bind the lookup
owner before dispatch. The constructor invokes that bound inherited hook once;
cooperative hooks own continuation through the MRO. No binder invents a missing
receiver or discards class-hook arguments. Default hooks reject leftover
positional and keyword arguments.

Direct `object.__new__` and `object.__init__` require their receiver. Extra
arguments are admitted only by the shared constructor policy: default new with
custom init, or default init with custom new, respectively. Ordinary allocation
and direct builtin calls share the resolved MRO facts; overriding both methods
does not authorize silently discarding arguments to either object builtin.

Regressions include runtime descriptor-surface and raw/bound admission tests plus
`tests/differential/basic/class_hook_binding.py`. These are contracts, not a
claim that every version/OS/architecture/backend cell has been executed.
`tests/differential/basic/class_hook_descriptor.py` separately preserves raw
`object.__dict__` classmethod-descriptor visibility/callability cases as an
unproven builtin-introspection frontier;
bound-hook proofs must not be reported as closure of those raw-descriptor cases.

### Shared C-extension calling conventions

The pointer-based CPython ABI uses
`molt-cpython-abi/src/api/cfunction.rs::CFunctionConvention` as the single
flag/arity/dispatch authority for raw `PyCFunction`/`PyCMethod` objects and
runtime-backed extension functions. It covers NOARGS, O, VARARGS with or without
keywords, FASTCALL with or without keywords, and METHOD/FASTCALL/KEYWORDS.
The boxed-value libmolt C API remains a distinct representation contract; its
function pointer signatures are not interchangeable with `PyObject *` signatures.

Both vectorcall and the runtime binder transport positional values followed by
keyword values, paired with an ordered keyword-name tuple. A kwargs dictionary
is never passed as FASTCALL kwnames. Only VARARGS conventions pack argument
tuples/dictionaries. Borrowed inputs remain pinned across extension reentry;
temporary argument cleanup preserves the callee's exact error indicator.

A runtime callable retains receiver, defining class and diagnostic name in its
ordinary traced closure. The executable registry owns no hidden Python edges.
METHOD requires an actual defining class; other conventions forbid one. STATIC
passes a null effective receiver while retaining the supplied object's lifetime.
Receiver presence is explicit: C NULL, Python None, and floating-point zero
cannot share an absence sentinel. Raw bridge bindings distinguish borrowed
identities from transferred runtime holds and retire the latter outside locks.
Direct aliases retire their own forward identity even when no reverse view
exists; they cannot retire a different canonical managed view. Static binding
rejections carry typed collision facts captured by the publication transaction.
Direct-ingress metadata is not ownership: an exact managed view takes precedence
over that metadata during release and cannot be rebound or unbound as a static
alias. C type readiness and capsule/descriptor construction acquire no hidden
runtime references. The sole C-to-runtime conversion, `molt_value_for_pyobj`,
preserves existing managed/static identity or acquires an owned foreign wrapper
on demand; its last runtime owner releases the wrapper's single C hold.
`RuntimeValue` is the shared temporary crossing guard for C-function owners,
module APIs, mapping/sequence mutation, and marshaled call arguments. It borrows
an already-observed canonical value or releases its temporary foreign owner
while preserving the exact error indicator. A failed managed observation cannot
be reclassified as foreign. Foreign reservation rechecks canonical identity
under the publication lock; allocation failure publishes neither a wrapper nor
a C hold. `PyModule_AddObject` consumes its C reference only on successful
insertion; `PyModule_AddObjectRef` never consumes the caller's reference.
Generic item access delegates to the mapping/sequence authorities rather than
maintaining a second conversion, result-ownership, or error policy.
Reference insertion uses `RuntimeValue::acquire_edge`: it validates physical
layout and commits initialized slots without requiring container construction
to be complete. Semantic reads use strict observation. This distinction applies
to list/tuple insertion, dict values and module values, including self/mutual
construction cycles; dictionary keys and call arguments remain observations.
Runtime-to-C borrowed arguments acquire new C references without consuming the
caller's runtime hold. Foreign get/set/call cleanup preserves the exact pending
error, and attribute deletion has explicit presence independent of value bits.
Typed owned/borrowed results use status alone for success: float `+0.0` has bits
zero and remains a value. Uninitialized runtime tuple slots use the existing
canonical missing singleton, never a scalar payload sentinel. Foreign call and
attribute results use that same typed status authority.
Invalid construction cannot publish a partial registry entry or fall back from
a failed runtime constructor to a raw callable.

Runtime keyword binding recognizes this executable authority directly instead
of inventing Python parameter names for a C function. ABI object calls and
runtime calls preserve their respective result/error boundary validation.
Regressions live in ABI `test_cfunction_conventions` and runtime
`cpython_abi_hooks/cfunction_tests.rs`; host tests alone are not native/WASM,
Python-version, platform, or real-package acceptance.

### 5.1 Tier 0: Specialized, allocation-free call paths

Tier 0 aims to produce direct Cranelift calls that:
- Pass positional arguments in registers/stack slots directly.
- Pass keyword args as a compact, read-only keyword table (no dict allocation).
- Avoid allocating intermediate tuples/dicts for common cases.

Tier 0 accepts a call site into Tier 0 only if:
- The callee is statically known (or guarded with a stable identity guard) and has a known Molt signature.
- Keyword names are statically known (compile-time strings).
- Any `*` / `**` expansions can be proven to be:
  - **flattenable** (compile-time known length / keys), or
  - **guardable** into a small set of known shapes.

#### 5.1.1 Tier 0 call lowering classes

**Class A: Direct positional call (fastest)**
- No keywords, no `*`, no `**`.
- `npos` matches a specialized callee entry (exact or with defaults applied at compile time).

**Class B: Direct call with compile-time keywords**
- Keywords present, but all keyword names are compile-time constants and no `**`.
- Lower to a “vectorcall-like” ABI:
  - `args_ptr, npos, kwnames_ptr, nkw` passed to a specialized callee binder stub.
- No dict allocation.

**Class C: Flattenable `*` / `**`**
- `*` expansion over tuple/list literals with compile-time known element count, or over statically known fixed-size tuples.
- `**` expansion over dict literals with compile-time constant string keys.
- Compiler expands them into Class A or B forms at compile time.

**Class D: Guarded call-shape specialization**
- `*`/`**` expansions exist but are proven (via Type Facts Artifact or analysis) to have a small set of shapes (e.g., tuple length <= 4, or dict keys in a known set).
- Emit guard(s) and route to specialized stubs; fallback to Tier 1 binder on mismatch.

### 5.2 Tier 1: Generic binder

Tier 1 is the correctness backstop:
- Evaluates arguments and expansions.
- Builds a runtime `CallArgs` buffer.
- Performs binding with signature metadata.
- Allocates `*args` tuple and/or `**kwargs` dict at function entry if required by signature.
- Produces correct `TypeError`s.

Tier 1 can be used:
- When call shape is dynamic or too complex.
- When `*` / `**` inputs are not in the Tier 0 allowlist.
- When the callee is not statically known or is a dynamic callable.

Tier 1 can still be compiled (Cranelift), but is treated as a slower path.

---

## 6. IR Design

### 6.1 HIR

Add explicit argument nodes that preserve source order:
- `CallArg::Pos(expr)`
- `CallArg::Star(expr)`   // `*expr`
- `CallArg::Kw(name, expr)` // `name=expr`
- `CallArg::KwStar(expr)` // `**expr`

HIR retains the exact ordering of these items for correct evaluation.

### 6.2 TIR

Introduce:
- `CallShape` type metadata on call ops:
  - `npos_static: Option<u16>`
  - `kwnames_static: Option<Vec<StrId>>`
  - `has_star: bool`
  - `has_kwstar: bool`
- New (conceptual) TIR instructions:
  - `BuildCallArgs { inline_capacity_pos, inline_capacity_kw } -> CallArgsHandle`
  - `PushPos(CallArgsHandle, Value)`
  - `PushKw(CallArgsHandle, StrId, Value)`
  - `ExpandStar(CallArgsHandle, Value)`      // iterates or flattens
  - `ExpandKwStar(CallArgsHandle, Value)`    // iterates mapping, checks str keys
  - `CallDirect { callee_fn, values... }`
  - `CallVector { callee_fn, args_ptr, npos, kwnames_ptr, nkw }`
  - `CallBind { callee_fn, CallArgsHandle }` // Tier 1 binder path

Lowering chooses among these based on Tier rules.

### 6.3 LIR

- Tier 0 `CallDirect` becomes a direct machine call with N fixed arguments.
- Tier 0 `CallVector` lowers to passing:
  - pointer to contiguous `MoltValue` array (positional)
  - count
  - pointer to contiguous `StrId` array (keyword names)
  - pointer to contiguous `MoltValue` array (keyword values)
- Tier 1 `CallBind` lowers to runtime `molt_bind_and_call(...)`.

---

## 7. Runtime Design

### 7.1 Core structs

```rust
/// A compact, non-Python “argument buffer” used during binding.
/// Prefer stack allocation + small-vector optimization.
pub struct MoltCallArgs<'a> {
    pub pos: &'a [MoltValue],
    pub kw_names: &'a [StrId],   // length == kw_values.len()
    pub kw_values: &'a [MoltValue],
    pub has_star: bool,
    pub has_kwstar: bool,
}
```

When building dynamically (Tier 1), use an owned smallvec-backed variant:

```rust
pub struct MoltCallArgsOwned {
    pub pos: smallvec::SmallVec<[MoltValue; 8]>,
    pub kw_names: smallvec::SmallVec<[StrId; 8]>,
    pub kw_values: smallvec::SmallVec<[MoltValue; 8]>,
}
```

### 7.2 Signature metadata

Each compiled function carries a `MoltSignature`:

```rust
pub struct MoltSignature {
    pub pos_only: u16,
    pub pos_or_kw: u16,
    pub kw_only: u16,
    pub has_varargs: bool,
    pub has_varkw: bool,

    /// Parameter name IDs for matchable params (pos-or-kw + kw-only).
    /// pos-only names are stored for error reporting but are not keyword-matchable.
    pub name_ids: &'static [StrId],

    /// Default values for the trailing subset of positional params and kw-only params.
    pub defaults: &'static [MoltValue],
}
```

For fast keyword matching, `name_ids` SHOULD be paired with a precomputed lookup table:
- Small N: linear scan
- Medium N: sorted array + binary search
- Large N: perfect hash / hash table (compile-time emitted)

### 7.3 Binding API

Tier 1 runtime entrypoint:

```rust
pub fn molt_bind_and_call(
    callee: MoltFunctionHandle,
    args: &MoltCallArgsOwned,
) -> MoltResult<MoltValue>;
```

Tier 0 vectorcall-style entrypoint (used by specialized stubs):

```rust
pub fn molt_vectorcall(
    callee: MoltFunctionHandle,
    pos: *const MoltValue,
    npos: usize,
    kw_names: *const StrId,
    kw_values: *const MoltValue,
    nkw: usize,
) -> MoltResult<MoltValue>;
```

The compiler may inline or specialize parts of binding for small shapes, but the runtime API remains the canonical fallback.

### 7.4 Allocation policy

- Tier 0:
  - No allocations for passing args/kwargs to the callee binder.
  - Allocation only occurs if the callee signature demands materialized `*args` / `**kwargs` locals.
- Tier 1:
  - Argument buffer uses SmallVec; spills allocate if the call is large.
  - `*args` tuple and/or `**kwargs` dict allocated at entry when required.

---

### 7.5 Callable ownership and metadata publication

`builtins/methods/common.rs` owns runtime builtin callable construction, defaults,
and signature setup. An atomic cache owns one reference; its readers borrow it.
Allocation failure returns zero with a pending exception and leaves the cache
unset. Python `None` is never a failed callable cache entry. Classless bootstrap
callables retain their explicit bootstrap boundary but use the same metadata
operations. Optional method dispatch distinguishes an absent name from a pending
failure; a zero allocation result must not become an ordinary scalar method.

`call/class_init.rs` owns function dictionary allocation and string-keyed
metadata writes. The first dictionary is prepared before publication; allocation
failure must neither dereference a null pointer nor install a partial dictionary.
An existing dictionary keeps its identity. Internal metadata writes and user
set/delete operations commit binder/task facts before releasing displaced
references; user defaults changes also invalidate compiled default guards before
a finalizer can reenter the function. Projection of those facts does not allocate
or call Python. Callback-capable task flags remain unevaluated until the call
boundary, where normal truth semantics apply.

The `types`, `functools`, and `operator` runtime classes use
`types::init_cached_runtime_class`: class edge, base, layout, complete method
signatures, namespace entries, and definition finalization precede cache
publication. Narrow source checks or one native test receipt do not establish
CPython-version, OS/architecture, or WASM matrix conformance.

### 7.6 Typed function fields and user dictionaries

Python-visible typed fields (`__defaults__`, `__kwdefaults__`, `__name__`,
`__qualname__`, `__doc__`, `__code__`, `__globals__`, and `__closure__`) do not
derive their values from equally named entries in the ordinary function
`__dict__`. Updating, clearing, or replacing that dictionary must not alter
execution, defaults-version admission, or typed descriptor reads. Conversely,
typed assignment/deletion must not edit the user's same-named dictionary key.

The conformance capsule
`tests/differential/basic/function_metadata_dictionary_isolation.py` covers these
ownership boundaries. CPython 3.12, 3.13, and 3.14 agree on its output. Molt's
current shared metadata dictionary does **not** satisfy this contract; the
callable-publication tests alone do not close this surface.

Closure requires one typed function payload, one GC/reference-field traversal,
and one retain/publish/release transaction for related field changes. Python
signature facts belong to the code object; ordinary `__molt_*` dictionary keys
must not be executable signature or task-classification authority. Binder,
call-cache, constructor, metadata ABI, and frontend producers must migrate
together; a dictionary-epoch workaround would preserve the wrong semantics.

## 8. Compatibility & Restrictions

### 8.1 Verified subset rules (initial)
- Tier 0 allowlist:
  - `*expr`: tuple/list literal, tuple/list value with trusted “small-length” facts, or `range` (optional).
  - `**expr`: dict literal with constant string keys, or dict value with trusted “small keyset” facts.
- Tier 1 supports:
  - Any Molt `list`/`tuple` iterable for `*expr` (may reject custom iterators initially).
  - Dict (and dict-like) mappings for `**expr` where keys are `str`.

### 8.2 Error behavior
- Molt must raise `TypeError` for invalid binding.
- For differential testing, error class and (where practical) the offending parameter name must match CPython. Exact wording is a later hardening step.

---

## 9. WASM Considerations

- The call binding model is internal to Molt and should work identically when Molt targets WASM.
- If the WASM backend restricts stack usage, `MoltCallArgsOwned` should cap inline sizes and spill to linear memory buffers deterministically.
- No WIT surface changes are required for basic in-language calling semantics; WIT matters for Molt↔Package calls (`@molt.ffi`) and is orthogonal.

---

## 10. Testing Plan

### 10.1 Differential tests (must-have)
Add a new differential suite directory, e.g. `tests/differential/calls/`, covering:

- Positional binding:
  - `def f(a,b): ...; f(1,2)`
  - too many args
- Defaults:
  - `def f(a,b=2): ...; f(1)`
- Keyword binding:
  - `def f(a,b): ...; f(b=2,a=1)`
  - missing required
- Duplicates:
  - `f(1,a=2)` (multiple values)
  - `f(a=1, **{'a': 2})`
- Varargs:
  - `def f(a,*args): ...; f(1,2,3)`
- Varkw:
  - `def f(**kw): ...; f(a=1,b=2)`
  - unexpected keyword when no varkw
- Keyword-only:
  - `def f(*, a): ...; f(a=1)` and `f(1)` error
- Positional-only (if parser supports):
  - `def f(a, /, b): ...; f(a=1, b=2)` error
- `*` expansion:
  - `f(*[1,2])`, `f(*(1,2))`
- `**` expansion:
  - `f(**{'a':1})`, non-str key error

### 10.2 Unit tests (runtime)
- `molt_bind_and_call` binder correctness for edge cases:
  - large keyword tables
  - mixed pos/kw with defaults
  - insertion-order correctness for varkw dict

### 10.3 Performance tests
Add (or extend) a microbench:
- `bench_call_overhead.py`:
  - direct positional calls
  - keyword calls with small fixed keyword tables
  - varargs calls
  - calls using `*`/`**` expansions

Performance gate target (initial):
- Tier 0 direct calls must not regress.
- Keyword calls should be within ~1.2–1.5× of direct positional calls for small `nkw` (goal; tune after baseline).

---

## 11. Rollout Plan

### Phase 0: Keywords without expansion
- Support keyword args at call sites and binding in callee.
- No `*`/`**` expansions yet.

### Phase 1: `*args` / `**kwargs` (basic)
- Implement expansions in Tier 1 binder.
- Enable Tier 0 flattening for tuple/list/dict literals.

### Phase 2: Call-shape specialization
- Add guarded stubs for hot shapes and TFA-driven specialization.

### Phase 3: Advanced optimizations (optional)
- Lazy materialization of `*args` tuple / `**kwargs` dict when unused (requires escape analysis and careful semantics).
- Perfect-hash keyword matching for large APIs.

---

## 12. Open Questions

- Do we require exact CPython `TypeError` messages for parity (string equality), or only class + key name?
- Which mapping protocol subset should Tier 1 support for `**expr` beyond dict?
- Should Tier 0 accept `*expr` for `range` and `memoryview` once those types are stabilized?
- How aggressively should we specialize by keyword set (risk: code size explosion)?

---

## 13. References (Repo-local)

- `docs/spec/areas/core/0002-architecture.md` (IR stack and pipeline)
- `docs/spec/areas/runtime/0003-runtime.md` (object model + runtime constraints)
- `docs/spec/areas/wasm/0005-wasm-interop.md` (WASM strategy)
- `docs/spec/areas/tooling/0012_MOLT_COMMANDS.md` (TFA and tooling hooks)
- `docs/spec/STATUS.md` (canonical current status)
- `ROADMAP.md` and `OPTIMIZATIONS_PLAN.md` (priorities and perf gating patterns)
