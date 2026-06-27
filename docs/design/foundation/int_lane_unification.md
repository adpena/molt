# Int-Lane Unification: One Carrier Authority

Status: DESIGN (ready_to_implement; lands in the build slot freed by the modulo point fix)
Scope: the raw-i64-vs-boxed carrier-classification fragmentation that produced the
loop-IV modulo P0 (silent wrong answer), and its permanent structural cure.
Companion: [[loop-iv-modulo-carrier-bug]] (the instance), the scalar-repr
canonicalization keystone (the typed `Repr` lattice this builds on).

---

## 0. Thesis

The loop-IV modulo P0 (`print(i % 7)` → NaN-box bits as a raw i64) is **not** a
modulo bug. It is one instance of a **fragmented carrier authority**: at least five
independent sites each decide whether a value's SSA name is a raw-i64 carrier or a
boxed value, and they can disagree. `value_range.rs` proves `i % 7 ∈ [0,7)` (raw-safe),
but the native mod lane and the consumer-boxing helpers re-decide independently from
the name's `repr_by_name` entry, so the result is stored under one assumption and read
under the other. The cure is to make the carrier classification a **single fact**,
computed once in the representation plan and **read** (never re-decided) by every
consumer — so a producer/consumer disagreement is unexpressible, exactly as the
`op_family` single-source-of-truth gate made dispatch↔handler drift unexpressible.

---

## 1. The five fragmented decision sites (verified file:line)

1. **Value-range producer** — `runtime/molt-passes/src/tir/passes/value_range.rs:1297`
   (`ValueRangeTransferRule::Mod` → `IntRange::mod_const`): proves the result's bounds.
2. **Registration authority** — `representation_plan.rs:518` (`is_raw_int_carrier_name`):
   reads the name-keyed `repr_by_name` map.
3. **Arith lanes** — `arith_division.rs:684` (mod lane checks `is_raw_int_carrier_name(out)`
   to branch raw vs boxed); siblings in `arith*.rs`, `const_literals.rs:423`, `loops.rs:478`.
4. **Boxing helpers** — `scalar_carriers.rs:41-55` (`def_inline_int_value`): reads the
   predicate to choose the register target.
5. **Boxing recovery** — `scalar_carriers.rs:253` (`var_get_boxed_overflow_safe_base`):
   reads the predicate to decide extraction.

The defect is structural: the producer (value-range proof) and the consumers (lanes,
helpers) reach the carrier decision by **different paths**, and nothing forces them to
agree. `mod_const` proving raw-safe does not guarantee `repr_by_name[out]` is raw — so
the fast raw path can be skipped (or, worse, a boxed value read as raw → the P0).

---

## 2. The unified authority

**Single source of truth: the typed `Repr` lattice in `representation_plan/value_repr.rs`.**

```
VALUE-RANGE PROOF  (value_range::fits_inline_int47 — bounds only; NO carrier decision)
        │  produces IntRange facts; consumed by BCE independently
        ▼
REPRESENTATION PLAN  (value_repr.rs raw_i64_safe_value_seed @ ~332 + phi all-incomings)
        │  the ONE place fits_inline_int47 is turned into Repr::RawI64Safe
        ▼
REPR REGISTRY  (ScalarRepresentationPlan.repr_by_name [native] / LlvmReprFacts.repr_by_value [llvm/wasm/rc])
        │  finalized once after plan construction; immutable during lowering
        ▼
ALL CONSUMERS READ HERE  (is_raw_int_carrier_name → repr_by_name[name].is_raw_i64_carrier())
   arith lanes · boxing def · boxing recovery · const literals · loop range · print/str
```

**Separation of concerns (load-bearing):**
- **Value-range** only *produces* `IntRange` + `fits_inline_int47`. It MUST NOT decide
  carrier classification. (Conflating the two is exactly how `mod_const`'s RawI64Safe
  range leaked into a carrier decision.)
- **BCE** consumes `fits_inline_int47` *independently* of the carrier authority — bound-
  check-elimination and representation are decoupled (#20).
- **Representation plan** is the sole minter of `Repr::RawI64Safe`, seeded from
  `fits_inline_int47` + the phi all-incomings rule, written once into the registry.
- **Every consumer** reads the registry via `is_raw_int_carrier_name`. No site re-derives
  raw-vs-boxed from anything but the registry.

---

## 3. Migration (each independent decision → read-only registry consumer)

| Site | Current | After |
|---|---|---|
| `value_range.rs:1297` `mod_const` | produces range AND implies raw | produces range ONLY (BCE input) |
| `value_repr.rs:~332` `raw_i64_safe_value_seed` | one of several seeders | THE seeder: `fits_inline_int47` → `Repr::RawI64Safe` |
| `representation_plan.rs:364/367/370` `repr_by_name` | populated from several paths | single build from `repr_by_value_for()` |
| `arith_division.rs:684` mod lane | `is_raw_int_carrier_name(out)` re-decides | reads registry only |
| `scalar_carriers.rs:49` boxing def | re-decides | reads registry only |
| `scalar_carriers.rs:253` boxing recovery | re-decides | reads registry only |
| `const_literals.rs:423`, `loops.rs:478` | re-decide | read registry only |

---

## 4. Why the modulo P0 (and its siblings) become unexpressible

Once `repr_by_name` is finalized, `is_raw_int_carrier_name(name)` returns the SAME
answer for a name on every call — the decision is serialized into an immutable map. The
producer (value-range → seed → registry) and every consumer read the identical
`Repr`. A site that stored boxed but read raw (the P0) cannot exist, because both the
store and the read consult the one registry entry. This generalizes across the int op
family — add/mul/floordiv/shift/mod all route their result carrier through the same
authority, so no sibling can reintroduce the disagreement.

---

## 5. CI gate (build-time, makes regression unexpressible)

- New `tests/test_representation_unification.rs::modulo_result_carrier_agreement`:
  build a TIR `result = a % large_positive_const`, assert
  `repr_plan.is_raw_int_carrier_name(out) == vr.fits_inline_int47(result)` — i.e. the
  registry agrees with the value-range proof. Extend with one case per int op.
- Lint/structural-audit rule: flag any codegen site that makes a raw-vs-boxed decision
  **not** rooted in `repr_by_name`/`repr_by_value` (the forbidden independent-decision
  pattern). Wire into `tools/ci_gate.py` so a future fragmenting edit fails CI.

---

## 6. Relationship to the modulo point fix

The in-flight modulo point fix (reconcile `mod_const`'s carrier at the one site) stops
the P0 bleeding fast. This unification is the **class** fix per the bug-class-not-
instance doctrine: it absorbs the point fix (the mod-lane site becomes a registry
reader) and forecloses the whole family. If the point fix landed as a clean registry
read it is a step toward this; if it landed as a mod-specific special-case it is
replaced here. Either way the end state is one authority, zero independent deciders.

---

## 7. Verification

- The modulo repro (`print(i % 7)` loop) and `alias_reassign_conditional_del.py` → correct,
  native + LLVM/WASM/Luau (the registry is the shared `Repr` lattice — backend-uniform).
- Full int/loop differential subset green; no perf regression on the numeric loop
  benchmarks (the raw lane must still fire — the registry says raw, the lane emits raw).
- The CI gate (§5) fails on a synthetic fragmenting edit (negative control).
