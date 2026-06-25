# INT-lane unification — native carrier migration record

Companion to `docs/design/int_lane_unification.md`. This is the complete survey
and implementation record for the STEP-4 atomic migration: the native backend no
longer threads a cloned `int_primary_vars: &BTreeSet<String>` carrier set.
Native int-carrier decisions now read the single `&ScalarRepresentationPlan`
authority directly through:

- `is_raw_int_carrier_name(name)` for "any raw i64 carrier" storage decisions.
- `is_inline_safe_int_name(name)` for inline-int47-safe box sites.
- `is_full_deopt_int_name(name)` for checked-overflow full-i64 box sites.

Current binding gate: `git grep int_primary_vars -- runtime/molt-backend` is
empty. The counts below are the pre-migration audit that made the atomic move
reviewable; they are retained as provenance, not as live instructions.

Native scalar-plan follow-through: the same handler-wide authority class now
also forbids cloned `bool_primary_vars` / `float_primary_vars` sets and the
legacy `int_carriers_plan` alias in native backend code. The live gate is
`tools/structural_audit.py --check`, metric
`native_scalar_plan_authority_violations = 0`, backed by direct scans of
`runtime/molt-backend/src/native_backend/function_compiler.rs` and
`runtime/molt-backend/src/native_backend/function_compiler/**`.

**Framing (binding):** this is ONE atomic threading change, NOT 41 chips. Every
file below is part of the same structural arc — the same arc the FLOAT-lane cut
executed for `float_primary_vars` ("all 41 `fc/` handler files +
`function_compiler.rs` as one structural arc, 1206 lines", float doc STEP 4).
Partial migration leaves two scalar-carrier authorities live, which the
CLAUDE.md asymmetry rule forbids. The map exists so no `fc/` site is missed and
so the change can be reviewed as one coherent diff.


## The authority that was unified

- **Previous source:** `function_compiler.rs:1700`
  `let int_primary_vars = primary_names.int;` where
  `primary_names = representation_plan.primary_name_sets()`
  (`function_compiler.rs:1699`) and `.int = self.int_carrier_names()`
  (`representation_plan.rs:1658,1669`). `int_carrier_names()` is the
  `{name | repr_by_name[name].is_raw_i64_safe()}` view.
- **The deleted clone:** that `BTreeSet<String>` was threaded by `&` into every
  handler and helper signature as `int_primary_vars: &BTreeSet<String>` and read
  via `int_primary_vars.contains(name)` (e.g. `fc/arith.rs:234,503,613`;
  `scalar_carriers.rs:37,60,258`).
- **Current target:** every old `int_primary_vars.contains(name)` read is now a
  read off the already-threaded `representation_plan` via a name-keyed
  predicate. The int cut adds these accessors next to the float cut's
  `is_float_unboxed(name)`:
  - `is_inline_safe_int_name(name) -> bool` — the `RawI64Safe` name view
    (`{name | repr_by_name[name] == RawI64Safe}`), the inline-47 carriers.
  - `is_full_deopt_int_name(name) -> bool` — the `RawI64FullDeopt` name view
    (`{name | repr_by_name[name] == RawI64FullDeopt}`), the checked-op carriers.
  - `is_raw_int_carrier_name(name) -> bool` — the combined raw-carrier view for
    native storage and raw-read decisions that do not need to distinguish box
    tiers.
- **Deletion (STEP 5):** complete. The `let int_primary_vars =
  primary_names.int` binding is gone from native backend code.

> **Why a name predicate, not the value-keyed `is_inline_safe_int(id)`.** The
> existing `is_inline_safe_int(&self, id: ValueId)`
> (`representation_plan.rs:1020`) is keyed on `ValueId`. The native backend's
> hot path is keyed on Variable *name* (`vars[name]`), so it needs the name-keyed
> predicate. This mirrors the float cut, which added `is_float_unboxed(name)`
> rather than reusing a value-keyed float query.


## Per-file action set

Counts are `git grep -c int_primary_vars` occurrences (declarations + reads +
forwarded args) as of HEAD `dd1ae4ee3`. Total = **1353 occurrences across 44
files**. Two STEP-4 actions:
- **DROP** — file already threads `representation_plan`; migration removes the
  `int_primary_vars` param + rewrites `.contains(name)` reads to the plan
  predicate. (9 `fc/` files, set by the float cut.)
- **ADD+DROP** — file does NOT yet thread `representation_plan`; migration adds
  the `representation_plan: &ScalarRepresentationPlan` param, threads it from the
  caller, rewrites reads, and drops `int_primary_vars`. (33 files.)

### Orchestrator + carrier hub (non-`fc/`)

| file | occ | action | notes |
| --- | ---: | --- | --- |
| `native_backend/function_compiler.rs` | 67 | SOURCE + DROP | owned the `let int_primary_vars = primary_names.int` binding (`:1700`), the var-type declaration loop (`:1863,1904,2238`), and forwarded `&int_primary_vars` into ~50 handler call sites (`:2589`…`:3858`). STEP 5 deleted the binding and rewrote var-type/scalar-slot decisions to `representation_plan.is_raw_int_carrier_name(name)`. |
| `native_backend/function_compiler/scalar_carriers.rs` | 45 | ADD+DROP | the carrier helper hub: `int_raw_value` (`:31`), `def_var_from_*` / boxed-carrier helpers (`:55,158,254,376,400`). Pre-migration, these all took `int_primary_vars: &BTreeSet<String>` and used `.contains(name)` (`:37,60,258`); they now take `&ScalarRepresentationPlan` and read plan predicates. This is the int analog of the float cut's `float_value_for`/`def_var_from_*` migration. |
| `native_backend/simple_backend.rs` | 4 | ADD+DROP | `ensure_boxed_overflow_safe` reads full-deopt from the plan, not a passed-in set (design STEP 4). |

### `fc/` handlers — already thread `representation_plan` (DROP param only)

| file | occ |
| --- | ---: |
| `fc/arith.rs` | 167 |
| `fc/compare.rs` | 49 |
| `fc/loops.rs` | 44 |
| `fc/control_flow.rs` | 37 |
| `fc/unary_logic.rs` | 34 |
| `fc/sequence_ops.rs` | 28 |
| `fc/indexing.rs` | 28 |
| `fc/dict_ops.rs` | 52 |
| `fc/mod.rs` | 4 |

> `fc/list_index_fast_path.rs` already threads `representation_plan` but does NOT
> declare `int_primary_vars` — no param to drop; it is listed here only so STEP 4
> reviewers know it is already on the plan and its bce-fast-path reads should
> consult `proves_index_in_bounds_conservatively`-derived `bce_safe` (set in TIR),
> not re-derive carrier facts.

### `fc/` handlers — need `representation_plan` ADDED (ADD+DROP)

| file | occ | | file | occ |
| --- | ---: | --- | --- | ---: |
| `fc/text_predicates.rs` | 99 | | `fc/modules.rs` | 22 |
| `fc/vec_reductions.rs` | 61 | | `fc/exceptions.rs` | 22 |
| `fc/text_transform.rs` | 60 | | `fc/memoryview_buffer.rs` | 21 |
| `fc/ret_jump.rs` | 60 | | `fc/class_ops.rs` | 19 |
| `fc/memory.rs` | 51 | | `fc/parse_ops.rs` | 18 |
| `fc/list_ops.rs` | 34 | | `fc/dataclass.rs` | 18 |
| `fc/calls.rs` | 32 | | `fc/future_promise.rs` | 16 |
| `fc/coroutine.rs` | 26 | | `fc/value_transfer.rs` | 15 |
| `fc/type_conversions.rs` | 25 | | `fc/object_construct.rs` | 14 |
| `fc/set_ops.rs` | 24 | | `fc/generators.rs` | 14 |
| `fc/attrs.rs` | 24 | | `fc/statistics.rs` | 13 |
| `fc/funcobj.rs` | 23 | | `fc/callargs.rs` | 12 |
| `fc/runtime_ops.rs` | 11 | | `fc/file_io.rs` | 11 |
| `fc/context_mgmt.rs` | 11 | | `fc/type_checks.rs` | 10 |
| `fc/scalar_builtins.rs` | 8 | | `fc/exception_stack.rs` | 7 |
| `fc/const_literals.rs` | 7 | | `fc/exception_control.rs` | 6 |

(32 `fc/` files in this group + `scalar_carriers.rs` listed above = 33 ADD+DROP.)

### Roll-up

| group | files | occ |
| --- | ---: | ---: |
| orchestrator (`function_compiler.rs`) | 1 | 67 |
| carrier hub + simple_backend | 2 | 49 |
| `fc/` DROP-only (plan already threaded) | 9 | 443 |
| `fc/` ADD+DROP | 32 | 794 |
| **total** | **44** | **1353** |

(`fc/` DROP-only occ = 167+49+44+37+34+28+28+52+4 = 443. `fc/` ADD+DROP occ =
794. Orchestrator+hub = 116. 443+794+116 = 1353.)


## Transformation pattern (one shape, applied everywhere)

Before (every handler/helper):
```rust
fn handle_x(
    …,
    int_primary_vars: &BTreeSet<String>,
    representation_plan: &ScalarRepresentationPlan,  // present in 9 fc/, absent in 33
    …,
) {
    if int_primary_vars.contains(name) { /* raw i64 carrier path */ }
}
```

After:
```rust
fn handle_x(
    …,
    representation_plan: &ScalarRepresentationPlan,  // threaded everywhere
    …,
) {
    if representation_plan.is_raw_int_carrier_name(name) { /* raw i64 carrier path */ }
    // …or is_full_deopt_int_name(name) / is_inline_safe_int_name(name) where the
    // call site must distinguish the inline-47 tier from the full-range tier
    // (box-site discipline: full-deopt boxes overflow-safe, inline-47 boxes inline).
}
```

The few sites that must distinguish the two tiers (rather than "is it any raw int
carrier") are the **box sites** and the **var-type / scalar-slot decisions** in
`function_compiler.rs` and `scalar_carriers.rs`: a `RawI64FullDeopt` value boxes
via the overflow-safe path, a `RawI64Safe` value inline-boxes (design STEP 6 /
box-site discipline). Every other read is the carrier-or-not predicate
`is_raw_int_carrier_name(name)`.


## Verification (post-migration)

- `cargo test -p molt-backend --features native-backend --lib` — full native
  suite green after the 44-file carrier migration.
- `cargo test -p molt-tir representation_plan --lib` — name-keyed and
  value-keyed raw-i64 tiers stay coherent after the `raw_i64` extraction.
- E2E `molt build` on an int-heavy program (the 44-file signature churn must
  build end-to-end, not just unit-compile) — mirrors the float cut using its 4
  float differentials as the E2E gate.
- BINDING: `git grep int_primary_vars -- runtime/molt-backend` == 0.
- Memory-Safety Gates 1 & 2 (the two differentials) green on native/WASM/LLVM.
