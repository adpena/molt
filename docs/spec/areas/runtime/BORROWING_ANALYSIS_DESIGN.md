# Perceus-Style Borrowing Analysis for Molt RC

> Design document. Priority #2 optimization. Expected impact: 15-25% RC operation reduction.
>
> Date: 2026-03-12. Status: Design (pre-implementation).

---

## Table of Contents

1. [Current RC Protocol Summary](#1-current-rc-protocol-summary)
2. [Problem Statement](#2-problem-statement)
3. [Perceus Borrowing Model](#3-perceus-borrowing-model)
4. [Molt-Specific Design](#4-molt-specific-design)
5. [Last-Use Analysis and Precise Drop Placement](#5-last-use-analysis-and-precise-drop-placement)
6. [CallArgs Ownership Transfer](#6-callargs-ownership-transfer)
7. [TIR-Level Annotations](#7-tir-level-annotations)
8. [Backend Changes](#8-backend-changes)
9. [Instruction Savings Estimates](#9-instruction-savings-estimates)
10. [Phased Rollout Plan](#10-phased-rollout-plan)
11. [Safety Invariants and Correctness](#11-safety-invariants-and-correctness)
12. [Non-Goals and Deferred Work](#12-non-goals-and-deferred-work)

---

## 1. Current RC Protocol Summary

### NaN-Boxing and RC-Free Values

Molt uses NaN-boxed 64-bit values (`MoltObject`). The following value types are encoded inline and **never touch the RC system**:

| Type | Encoding | RC behavior |
|------|----------|-------------|
| Float (non-NaN) | Raw f64 bits | No RC (inline) |
| Int (47-bit) | `QNAN \| TAG_INT \| payload` | No RC (inline) |
| Bool | `QNAN \| TAG_BOOL \| 0/1` | No RC (inline) |
| None | `QNAN \| TAG_NONE` | No RC (inline) |
| Pending | `QNAN \| TAG_PENDING` | No RC (inline) |

Only pointer-tagged values (`QNAN | TAG_PTR | addr`) require RC management. The runtime helpers `inc_ref_bits` / `dec_ref_bits` check `obj.as_ptr()` and no-op for non-pointer values.

The `emit_maybe_ref_adjust` helper in the backend unconditionally calls `local_inc_ref_obj` without branching on the tag -- it relies on the runtime function to no-op for non-pointer values. This means every accessor result pays a function call even for inline values, which is a target for static elimination.

### Heap Object RC Mechanics

Every heap object has a `MoltHeader` with a `MoltRefCount` (AtomicU32 on native, Cell<u32> on wasm32). The key operations:

- **`inc_ref_ptr`**: Skips null and immortal objects (`HEADER_FLAG_IMMORTAL`). Otherwise `fetch_add(1, Relaxed)`. Cost breakdown: GIL assertion, null check, header pointer arithmetic (ptr.sub(size_of::<MoltHeader>())), immortal flag check, atomic fetch_add.
- **`dec_ref_ptr`**: Skips null, `TYPE_ID_NOT_IMPLEMENTED`, and immortal. Otherwise `fetch_sub(1, AcqRel)`. If count reaches zero: acquire fence, optional finalizer (`__del__`), weakref clearing, then type-dispatched destructor (which recursively dec-refs children). The `AcqRel` ordering on dec_ref (vs `Relaxed` on inc_ref) is required for the zero-check to synchronize with the last writer.

Both ABI entry points (`molt_inc_ref_obj`, `molt_dec_ref_obj`) go through `with_gil_entry!`, which acquires a `GilGuard` (real lock on native, no-op on wasm32). This per-call GIL acquisition is a meaningful cost when RC calls dominate -- on native targets, even an uncontended mutex still involves an atomic compare-exchange.

### Exported ABI Functions (called from Cranelift-generated code)

- `molt_inc_ref_obj(bits: u64)` -- inc_ref if pointer, no-op otherwise.
- `molt_dec_ref_obj(bits: u64)` -- dec_ref if pointer, no-op otherwise.
- `molt_dec_ref(ptr: *mut u8)` -- dec_ref a raw pointer (skips the NaN-box check).

These are emitted as `call` instructions by the Cranelift backend. Each call crosses the ABI boundary (function call overhead + GIL acquisition via `with_gil_entry!`).

**Static call-site counts in `molt-backend/src/lib.rs`** (snapshot 2026-03-12):

| Call site type | Count | Description |
|----------------|-------|-------------|
| `call(local_inc_ref_obj, ...)` | 9 | Explicit inc_ref for closure captures, generator payloads, container stores |
| `call(local_dec_ref_obj, ...)` | 26 | Dec_ref at last-use, return cleanup, block drain |
| `call(local_dec_ref, ...)` | 23 | Dec_ref for raw-pointer tracked values (same semantics, skips NaN-box check) |
| `emit_maybe_ref_adjust(...)` | 8 | Inc_ref for accessor results (`function_closure_bits`, `get_attr_*`, struct field loads) |

Total: **66 static RC emission sites**. At runtime, the last-use and return-path dec_ref sites fire per variable per function exit, making dec_ref the dominant RC operation (est. 3-5x more dynamic dec_ref calls than inc_ref).

### Current Calling Convention

The backend treats function parameters as **caller-owned/borrowed**: the caller does not emit inc_ref when passing arguments, and the callee does not emit dec_ref for its parameters on exit. This is already a partial borrowing convention, but it is implicit and limited to direct calls.

The backend emits inc_ref for:
- Closure captures stored into generator/task objects (explicit ownership transfer)
- Values entering callargs builders (`molt_callargs_push_pos` calls `inc_ref_bits` internally)
- Return values from accessor functions like `function_closure_bits` (via `emit_maybe_ref_adjust`)
- Values stored into container fields (task/generator payloads)

The backend emits dec_ref for:
- All tracked variables at their last-use point (`drain_cleanup_tracked` / `drain_cleanup_entry_tracked`)
- All remaining tracked variables at function return (the `ret` / `ret_void` handlers drain both `tracked_obj_vars`/`tracked_vars` and per-block `block_tracked_obj`/`block_tracked_ptr`)
- The return value is excluded from the return-path cleanup (it is "donated" to the caller)

### Tracking Infrastructure

The backend maintains two parallel tracking systems:

1. **Entry-block tracking** (`tracked_obj_vars`, `tracked_vars`, `entry_vars`): Values defined in the entry block. Cleaned up eagerly when their `last_use` index is reached, but only while still in the entry block. Remaining values are cleaned up at return.

2. **Per-block tracking** (`block_tracked_obj`, `block_tracked_ptr`): Values defined in non-entry blocks (inside if/else/loop). Cleaned up at last-use within their block, or drained entirely at return.

Both use `compute_last_use()` which builds a `HashMap<String, usize>` mapping each variable name to the last op index that references it.

---

## 2. Problem Statement

### Redundant RC operations in the current system

Despite the implicit borrowing for direct-call parameters, many RC operations remain unnecessary:

**Problem A: Redundant inc_ref/dec_ref pairs across calls.**
When function `f` calls `g(x)`, the current protocol:
1. `f` holds a reference to `x` (tracked for cleanup)
2. `g` receives `x` as borrowed (no inc_ref on entry -- good)
3. But if `g` stores `x` into a data structure, `g` must inc_ref `x` to keep it alive
4. When `f` later drops `x` at last-use, it emits dec_ref -- even if `x` was the last use *before* the call

In many cases, `f` could donate its ownership of `x` to the call site, eliminating both the dec_ref in `f` and the inc_ref in `g` for the storage case.

**Problem B: CallArgs double-counting.**
The `molt_callargs_push_pos` function calls `inc_ref_bits(val)` for every positional argument. Then `callargs_dec_ref_all` decrements all of them on scope exit. If the caller already holds a reference that it would dec_ref after the call, this creates a redundant inc/dec pair per argument.

**Problem C: Accessor functions over-ref.**
Functions like `function_closure_bits` return a borrowed pointer into an existing object, but the backend unconditionally calls `emit_maybe_ref_adjust` (inc_ref) on the result, then later dec_refs it at last-use. If the parent object is guaranteed alive for the duration, the inc/dec pair is unnecessary.

**Problem D: Short-lived temporaries.**
Chained operations like `a.method().another()` create intermediate values that are inc_ref'd when created and dec_ref'd one instruction later. These temporaries could be treated as borrowed from the operation that produced them.

### Quantitative estimate

Based on typical Python code patterns (function calls dominate, most values are consumed once):
- **40-60% of inc_ref calls** are immediately followed by a dec_ref within the same basic block with no intervening aliasing.
- **20-30% of function parameters** are only read (never stored or returned) -- they are natural borrows.
- Each redundant inc_ref/dec_ref pair costs ~8-12 instructions (NaN-box tag check + pointer check + atomic fetch_add/sub + immortal check + GIL entry), or ~15-25ns.

---

## 3. Perceus Borrowing Model

The Perceus algorithm (Reinking et al., PLDI 2021) defines a precise borrowing discipline:

### Owned vs Borrowed Parameters

Every function parameter is classified as either:
- **Owned**: The callee receives ownership. It must dec_ref the parameter on all exit paths (unless it transfers ownership elsewhere).
- **Borrowed**: The callee receives a non-owning reference. The caller guarantees the value stays alive for the duration of the call. The callee must inc_ref if it needs to extend the lifetime (e.g., storing into a data structure).

### Key Insight: Most Parameters Are Borrowed

In practice, most function parameters are only *read* during the call -- they are not stored into mutable data structures or returned. Marking these as borrowed eliminates the inc_ref at the call site and the dec_ref at the callee's return.

### Ownership Transfer ("Donation")

When the caller's last use of a value is passing it to a call, and the callee's parameter is owned, the caller can *donate* its reference instead of inc_ref'ing for the call and dec_ref'ing after. This is a zero-cost ownership transfer.

### Interaction with Reuse Analysis

Perceus borrowing enables reuse analysis: when a `drop` (dec_ref reaching zero) immediately precedes an `alloc` of the same size, the memory can be reused. Precise drop placement (from borrowing analysis) maximizes the number of such drop-before-alloc pairs. This is out of scope for this design document but is enabled by it.

---

## 4. Molt-Specific Design

### 4.1. Borrow Classification Rules

A function parameter `p` can be marked **borrowed** if the function body satisfies all of these conditions:

1. `p` is never stored into a mutable container (list, dict, set, object attribute, closure capture, class field).
2. `p` is never returned from the function (returning transfers ownership to caller).
3. `p` is never stored into a global or module-level variable.
4. `p` is never passed as an **owned** argument to another function.
5. `p` is never yielded from a generator.
6. `p` is never captured by a closure that outlives the function scope.

If *any* of these conditions is violated on *any* control-flow path, `p` must be **owned**.

**Conservative default**: All parameters start as **owned**. The analysis proves borrowing is safe. This guarantees correctness -- the worst case is the status quo.

### 4.2. Value Linearity Classification

For each variable `v` in a function body, compute its **linearity class**:

| Class | Definition | RC action |
|-------|-----------|-----------|
| **Linear** | Exactly one use after definition | Drop at use point (no inc_ref on def, dec_ref donated or at use) |
| **Shared** | Multiple uses, all reads | Inc_ref to refcount = use_count; dec_ref at each use point |
| **Escaping** | Stored into heap, returned, or yielded | Must be owned; inc_ref at escape point |

For **linear** values that are passed to a function with a borrowed parameter, the entire inc_ref/dec_ref pair is eliminated. The caller simply ensures the value is alive (which it is, since it is in scope).

### 4.3. NaN-Box Aware Optimization

Since inline values (int, float, bool, None) never need RC, the analysis can skip them entirely. The type inference system already available in TIR provides type hints per variable. When a variable is known to be `int`, `float`, `bool`, or `None` at the TIR level, no RC operations are needed regardless of borrow/own classification.

This interacts with the existing `fast_int` / `fast_float` flags on `OpIR` -- values with these flags set can skip all RC emission.

### 4.4. Immortal Object Awareness

Constants (string literals, frozen modules, class objects) are immortal (`HEADER_FLAG_IMMORTAL`). The runtime already skips RC for immortals, but the backend still emits the *call* to `molt_inc_ref_obj` / `molt_dec_ref_obj`. A static analysis can identify variables that are provably immortal (loaded from constants, module globals that are class objects) and suppress RC calls entirely at compile time.

---

## 5. Last-Use Analysis and Precise Drop Placement

### 5.1. Current State

The backend's `compute_last_use()` already computes the last op index for each variable name. This is a basic last-use analysis over the linear op stream. However, it has limitations:

- It does not account for control flow: a variable used on only one branch of an `if` may have its "last use" index set to that branch's op, causing the dec_ref to be emitted only on that path. The entry-tracked cleanup handles this by falling back to return-level cleanup for non-entry-block values.
- It does not distinguish borrowed uses from consuming uses.

### 5.2. Proposed Enhancement: CFG-Aware Last-Use

Replace the linear `compute_last_use()` with a CFG-aware analysis that operates on basic blocks:

```
For each variable v:
  For each basic block B:
    compute last_use_in_block(v, B) = last op index in B that references v
    compute live_out(v, B) = true if v is used in any successor of B

  Drop point for v in B:
    if !live_out(v, B) and last_use_in_block(v, B) exists:
      emit dec_ref immediately after last_use_in_block(v, B)
    else:
      v remains live; do not drop in B
```

The `FunctionIR.ops` stream already encodes control flow via `if`/`else`/`end_if`/`loop_start`/`loop_end`/`br_if` ops, which the backend already parses to create basic blocks. The analysis can be performed as a pre-pass before codegen.

### 5.3. Drop Placement at Call Sites

When variable `v` has its last use as an argument to a call:
- If the corresponding parameter is **borrowed**: emit the dec_ref *after* the call returns (v must be alive during the call).
- If the corresponding parameter is **owned** and v is linear: *donate* the reference -- skip both the caller's dec_ref and the callee's dec_ref-on-return. The callee is now responsible for the reference.

This is the key optimization: donation at the last-use call site.

---

## 6. CallArgs Ownership Transfer

### 6.1. Current Protocol

The `call_bind` path (used for method dispatch, dynamic calls, kwarg calls):

1. Backend emits `molt_callargs_new(pos_capacity, kw_capacity)` -- allocates a CallArgs builder.
2. Backend emits `molt_callargs_push_pos(builder, val)` for each positional arg -- **this calls `inc_ref_bits(val)` internally**.
3. Backend emits `molt_call_bind(call_bits, builder_bits)` -- dispatches to the target function.
4. Inside `molt_call_bind`, a `PtrDropGuard` is created for the builder. On scope exit, `callargs_dec_ref_all` decrements all stored values.
5. `protect_callargs_aliased_return` handles the case where the return value aliases a stored argument (inc_refs the result to protect it from the PtrDropGuard's dec_ref).

### 6.2. Problem: Every Arg Gets an Extra inc_ref/dec_ref

For a 3-argument call via call_bind, the current protocol emits 6 unnecessary RC operations (3 inc_refs in push_pos + 3 dec_refs in callargs_dec_ref_all) that exist solely to keep the values alive during the call. But the *caller already holds references to these values* -- they are in scope.

### 6.3. Proposed: Borrowed CallArgs Mode

Introduce a **borrowed callargs** variant for the common case where all arguments are provably alive for the duration of the call:

**New runtime function**: `molt_callargs_push_pos_borrowed(builder_bits: u64, val: u64) -> u64`
- Same as `push_pos` but does **not** call `inc_ref_bits(val)`.

**New cleanup function**: `molt_callargs_drop_borrowed(args_ptr: *mut CallArgs)`
- Drops the CallArgs container itself (frees the Vec storage) but does **not** call `dec_ref_bits` on any stored values.

**Safety requirement**: The backend must guarantee that all values pushed via `push_pos_borrowed` remain alive (have at least one other reference) until `molt_call_bind` returns. This is guaranteed if the values are in the caller's tracked set and their last-use is at or after the call.

**Protect-aliased-return adjustment**: With borrowed callargs, `protect_callargs_aliased_return` behavior changes:
- Old: "If result aliases a callargs value, inc_ref the result (because callargs_dec_ref_all would dec_ref it)."
- New with borrowed mode: No protection needed -- the callargs dec_ref is not happening, so there is no double-free risk. The caller's tracking system handles the dec_ref of the original value.

However, this creates a new risk: the result might be the same bits as an argument, and the caller's tracking system might dec_ref the argument's variable name after the call. If the result is stored in a different variable name, it would be dec_ref'd separately, leading to a double-dec. This requires careful handling:

- If the result aliases an argument, and the argument's variable is being dropped after the call, the caller must **not** dec_ref the argument variable. Instead, the reference is logically transferred to the result variable.
- The backend can detect this: if the result of `call_bind` is used and argument `x` has its last-use at this call, and the result might alias `x`, the backend should remove `x` from the tracking set *without* emitting a dec_ref, and add the result to the tracking set.

**Fallback**: If the analysis cannot prove all arguments are alive, use the existing protocol. This is always safe.

### 6.4. IC Fast Path Integration

The `call_bind_ic_dispatch` path also uses `PtrDropGuard` and `protect_callargs_aliased_return`. The borrowed mode must be integrated here as well. Since the IC path reuses the same CallArgs builder, the borrowed flag should be stored on the CallArgs struct itself:

```rust
pub(crate) struct CallArgs {
    pos: Vec<u64>,
    kw_names: Vec<u64>,
    kw_values: Vec<u64>,
    kw_seen: HashSet<String>,
    borrowed: bool,  // NEW: if true, cleanup skips dec_ref
}
```

---

## 7. TIR-Level Annotations

### 7.1. New OpIR Fields

Extend `OpIR` with borrow annotations:

```rust
pub struct OpIR {
    // ... existing fields ...

    /// Per-argument borrow classification for call ops.
    /// `None` means all arguments are owned (default/conservative).
    /// `Some(vec)` where each element is true if the corresponding arg is borrowed.
    #[serde(default)]
    pub borrow_args: Option<Vec<bool>>,

    /// If true, the output of this op is a borrowed reference (caller must not dec_ref).
    /// Used for accessor ops that return pointers into existing objects.
    #[serde(default)]
    pub borrowed_result: Option<bool>,
}
```

### 7.2. New FunctionIR Fields

Extend `FunctionIR` with parameter borrow annotations:

```rust
pub struct FunctionIR {
    // ... existing fields ...

    /// Per-parameter borrow classification.
    /// `None` means all parameters are owned (default/conservative).
    /// `Some(vec)` where each element is true if the corresponding param is borrowed.
    #[serde(default)]
    pub borrow_params: Option<Vec<bool>>,
}
```

### 7.3. Frontend Emission

The Python frontend (`src/molt/frontend/__init__.py`) already emits `INC_REF`, `DEC_REF`, `BORROW`, and `RELEASE` ops. The borrowing analysis pass should:

1. Run after type inference (to know which values are inline/pointer types).
2. Compute borrow eligibility for each function parameter.
3. Compute linearity for each variable.
4. Annotate call ops with `borrow_args`.
5. Remove redundant `INC_REF`/`DEC_REF` ops that become unnecessary.
6. Insert `BORROW` ops for values that should be borrowed at call sites (the backend already handles `"borrow"` as equivalent to `"inc_ref"` -- this would change to a no-op tag).

### 7.4. Analysis Pass Location

The analysis should run as a **TIR-to-TIR transformation pass** after type specialization but before LIR lowering:

```
Python AST -> HIR -> TIR -> TIR (type-specialized) -> TIR (borrow-annotated) -> LIR -> Cranelift
```

This ensures type information is available (to skip inline-type variables) and the annotations flow through to the backend.

---

## 8. Backend Changes

### 8.1. Parameter Handling

When `borrow_params` is present on a `FunctionIR`:

- **Borrowed params**: Do not add to tracked cleanup sets. No dec_ref on function exit. The caller guarantees liveness.
- **Owned params**: Current behavior (add to tracked sets, dec_ref on exit).

This is nearly free since the backend already excludes params from call-site cleanup via `param_name_set`. The change is to also exclude borrowed params from return-path cleanup.

### 8.2. Call-Site Emission

When `borrow_args` is present on a `call` or `call_bind` op:

For each argument `arg_i`:
- If `borrow_args[i]` is true: the callee treats this param as borrowed. The caller does **not** emit an inc_ref for the call. If `arg_i`'s last use is this call, the caller emits dec_ref **after** the call returns (to maintain liveness during the call).
- If `borrow_args[i]` is false: current behavior (owned transfer).

For last-use donation (owned parameter + last use of variable is this call):
- Remove the variable from the tracking set *without* emitting dec_ref.
- The reference is donated to the callee.

### 8.3. Accessor Result Handling

When `borrowed_result` is true on an op:
- Do **not** call `emit_maybe_ref_adjust` (inc_ref) on the result.
- The result is a borrowed reference; it must not be added to tracking sets for dec_ref.
- Instead, the result inherits liveness from its source object (which must be in scope).

This eliminates the inc_ref/dec_ref pair for accessor functions like `function_closure_bits`, `bound_method_self_bits`, etc., when the parent object is provably alive.

### 8.4. CallArgs Mode Selection

At each `call_bind` emission site, the backend checks:
1. Are all positional arguments' variables still alive (last_use >= current op index)?
2. Is no keyword argument involved (kw args have different ownership dynamics)?

If both conditions hold, emit `molt_callargs_push_pos_borrowed` instead of `molt_callargs_push_pos`, and pass a flag to indicate borrowed mode.

---

## 9. Instruction Savings Estimates

### Per Direct Call Site (Borrowed Parameters)

For a call with `N` borrowed parameters:

| Operation | Current cost | With borrowing | Savings |
|-----------|-------------|----------------|---------|
| inc_ref per borrowed arg (call entry) | 0 (already borrowed) | 0 | 0 |
| dec_ref per borrowed arg (callee exit) | 0 (already not emitted) | 0 | 0 |
| dec_ref per borrowed arg (caller, at last-use if last-use = call) | ~10 insns * N | 0 (donated or deferred) | ~10N insns |

For a typical 2-parameter function where both params are borrowed and their last use is the call: **~20 instructions saved**.

### Per call_bind Site (Borrowed CallArgs)

For a call_bind with `N` positional arguments:

| Operation | Current cost | With borrowing | Savings |
|-----------|-------------|----------------|---------|
| `inc_ref_bits` in push_pos (per arg) | ~12 insns * N | 0 | ~12N insns |
| `dec_ref_bits` in callargs_dec_ref_all (per arg) | ~12 insns * N | 0 | ~12N insns |
| protect_callargs_aliased_return | ~8 insns | 0 | ~8 insns |

For a typical 3-argument method call via call_bind: **~80 instructions saved**.

### Per Accessor Result (Borrowed Return)

| Operation | Current cost | With borrowing | Savings |
|-----------|-------------|----------------|---------|
| `emit_maybe_ref_adjust` (inc_ref) | ~10 insns | 0 | ~10 insns |
| dec_ref at last-use | ~10 insns | 0 | ~10 insns |

Per accessor call: **~20 instructions saved**.

### Aggregate Estimate

In typical Python code with function calls averaging every 5-8 ops:
- ~40% of call sites eligible for borrowed params (pure read patterns): **saves ~20 insns * 0.4 * call_frequency**
- ~60% of call_bind sites eligible for borrowed callargs: **saves ~80 insns * 0.6 * call_bind_frequency**
- ~30% of accessor results eligible for borrowed result: **saves ~20 insns * 0.3 * accessor_frequency**

**Expected total: 15-25% reduction in RC-related instructions**, translating to approximately 5-10% wall-clock improvement on call-heavy code (where RC overhead is currently 20-40% of total execution time).

---

## 10. Phased Rollout Plan

### Phase 1: Parameter Borrow Inference (2-3 weeks)

**Scope**: Infer which function parameters are borrowed for Molt-compiled functions.

1. Implement borrow eligibility analysis as a TIR pass.
2. Add `borrow_params` field to `FunctionIR`.
3. Backend: skip return-path cleanup for borrowed params.
4. Validation: differential tests must pass. RC counts (via debug counters) should decrease.

**Risk**: Low. This is purely additive; the conservative default (all owned) preserves current behavior.

### Phase 2: Last-Use Donation for Direct Calls (1-2 weeks)

**Scope**: When a variable's last use is passing it to a direct call with an owned parameter, donate the reference.

1. Enhance `compute_last_use` to be CFG-aware.
2. At call emission, check if argument is last-use; if so, remove from tracking without dec_ref.
3. Validation: differential tests + RSS measurement to verify no leaks.

**Risk**: Medium. Incorrect donation leads to use-after-free. Must be gated on provably-correct last-use analysis.

### Phase 3: Borrowed CallArgs (2-3 weeks)

**Scope**: Implement the borrowed callargs mode for call_bind.

1. Add `borrowed` flag to `CallArgs`.
2. Implement `push_pos_borrowed` and `drop_borrowed`.
3. Backend: emit borrowed mode when all args are provably alive.
4. Adjust `protect_callargs_aliased_return` for borrowed mode.
5. Validate against the known aliased-return edge case from the Feb 2026 fix.

**Risk**: Medium. The callargs aliased-return fix is delicate. Must have specific test coverage for the kwonly-param use-after-free scenario in both borrowed and owned modes.

### Phase 4: Borrowed Accessor Results (1 week)

**Scope**: Mark accessor ops as returning borrowed references when the parent is alive.

1. Identify accessor ops: `function_closure_bits`, `bound_method_self_bits`, `bound_method_func_bits`, `class_name_bits`, `class_dict_bits`, etc.
2. Suppress `emit_maybe_ref_adjust` for these when source is in scope.
3. Track borrowed results separately (not added to dec_ref tracking).

**Risk**: Low. These are simple patterns with clear liveness guarantees.

---

## 11. Safety Invariants and Correctness

### Invariant 1: Borrowed values must be alive

A borrowed parameter or result must have at least one owning reference elsewhere that is guaranteed to outlive the borrow scope. The analysis must prove this statically. If in doubt, classify as owned.

### Invariant 2: No double-free

When donation transfers ownership from caller to callee, exactly one dec_ref must eventually execute for the value. The caller must not dec_ref (it removed the variable from tracking), and the callee must dec_ref on all exit paths (it is an owned parameter).

### Invariant 3: CallArgs aliased-return safety

The `protect_callargs_aliased_return` fix (Feb 2026) addressed use-after-free when a callee returns a value that aliases a callargs entry. In borrowed mode, the callargs does not dec_ref, so the aliasing is safe. But the caller's tracking system must correctly handle the result variable potentially aliasing an argument variable. The backend must not dec_ref both the argument and the result if they are the same bits.

### Invariant 4: Generator/coroutine suspension safety

Generator functions that yield must treat all live variables as potentially escaping (the generator frame holds them across yield points). The borrowing analysis must mark all parameters as owned in generator functions, or more precisely, treat yield points as escaping operations for all live variables.

### Invariant 5: Exception safety

If a function raises an exception, all owned parameters and local variables must still be cleaned up. The backend's return-path cleanup already handles this (exceptions route through the return block). Borrowed parameters are safe because the caller retains ownership.

### Testing Strategy

1. **Differential tests**: Must pass with identical output. RC optimization is invisible to semantics.
2. **RC debug counters**: Add `MOLT_DEBUG_RC_STATS=1` mode that counts inc_ref/dec_ref calls. Verify the count decreases with borrowing enabled.
3. **Valgrind/ASan**: Run differential suite under AddressSanitizer to detect use-after-free and leaks.
4. **Specific edge cases**: The Feb 2026 kwonly-param aliased-return test case must be included in every phase's validation.

---

## 12. Non-Goals and Deferred Work

### Out of scope for this design

- **Reuse analysis** (Perceus FBIP): When a drop immediately precedes an alloc of the same type/size, reuse the memory. This builds on precise drop placement from this design but is a separate optimization. Tracked as Priority #7 in OPTIMIZATION_ROADMAP_P1.md. Note that the runtime already has a **bucketed object pool** (`OBJECT_POOL_TLS` / `object_pool`) that recycles allocations up to 1024 bytes for `TYPE_ID_OBJECT`, `TYPE_ID_BOUND_METHOD`, and `TYPE_ID_ITER` types. Reuse tokens from Perceus would generalize this mechanism: instead of returning freed objects to a pool and later retrieving from the pool, a reuse token would allow the compiler to directly pass the freed memory to the next allocation of the same size class, eliminating the pool lookup overhead entirely.

- **Drop specialization**: Generating per-type destructors that know the exact layout. This is orthogonal to borrowing.

- **Biased reference counting**: Multi-threaded RC optimization. Molt currently uses the GIL; this becomes relevant when/if the GIL is removed.

- **Escape analysis**: Scalar-replacing heap objects that don't escape. This is Priority #3 (Partial Escape Analysis) and is complementary to but independent of borrowing.

- **Cross-module borrowing**: This design covers intra-module (single compilation unit) analysis. Cross-module borrowing requires link-time annotation propagation and is deferred.

---

## Appendix A: Key Source Files

| File | Role |
|------|------|
| `runtime/molt-runtime/src/object/mod.rs` (lines 363-375, 875-990) | `inc_ref_bits`, `dec_ref_bits`, `inc_ref_ptr`, `dec_ref_ptr` |
| `runtime/molt-runtime/src/object/refcount.rs` | `MoltRefCount` (AtomicU32/Cell<u32>) |
| `runtime/molt-runtime/src/object/ops.rs` (lines 42906-42921) | `molt_inc_ref_obj`, `molt_dec_ref_obj` ABI exports |
| `runtime/molt-runtime/src/call/bind.rs` | `CallArgs`, `callargs_dec_ref_all`, `protect_callargs_aliased_return`, `molt_call_bind`, `molt_call_bind_ic` |
| `runtime/molt-backend/src/lib.rs` (lines 839-948) | `compute_last_use`, `drain_cleanup_tracked`, `collect_cleanup_tracked` |
| `runtime/molt-backend/src/lib.rs` (lines 1773-1775) | `block_tracked_obj`, `block_tracked_ptr`, `last_use` initialization |
| `runtime/molt-backend/src/lib.rs` (lines 9094-9117) | Direct call emission with arg cleanup |
| `runtime/molt-backend/src/lib.rs` (lines 13688-13812) | `ret` handler with full tracking drain |
| `runtime/molt-backend/src/lib.rs` (lines 14015-14067) | Entry-tracked cleanup loop |
| `runtime/molt-obj-model/src/lib.rs` | NaN-boxing constants (`QNAN`, `TAG_INT`, `TAG_PTR`, etc.) |
| `src/molt/frontend/__init__.py` (lines 8980-8998) | `_emit_inc_ref`, `_emit_dec_ref`, `_emit_borrow`, `_emit_release` |

## Appendix B: References

- Reinking, Lorenzen, Leijen, Swierstra. "Perceus: Garbage Free Reference Counting with Reuse." PLDI 2021.
- Ullrich, de Moura. "Counting Immutable Beans: Reference Counting Optimized for Purely Functional Programming." IFL 2019 / arXiv 2019.
- Lorenzen. "Optimizing Reference Counting with Borrowing." Master's Thesis, 2023.
- Lorenzen, Leijen. "Reference Counting with Frame-Limited Reuse." ICFP 2022.
- PEP 703: Making the GIL Optional in CPython (biased reference counting).
