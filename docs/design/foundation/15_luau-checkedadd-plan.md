<!-- Parity recon (wf_971517d5-6b2, 2026-06-04). -->

# Luau Lowering Plan for `OpCode::CheckedAdd`

**Status (2026-06-23): implemented.** Current authority: `runtime/molt-backend/src/luau.rs`
emits `checked_add` via the conditional `molt_checked_i64_add` helper,
`runtime/molt-passes/src/tir/lower_to_simple.rs` round-trips `OpCode::CheckedAdd`
as SimpleIR `checked_add`, `test_compile_checked_lowers_checked_add_helper`
guards checked Luau emission, and
`docs/spec/areas/compiler/luau_support_matrix.generated.md` lists `checked_add`
as `implemented-exact`. The original recon below is retained for the semantic
rationale; line anchors and "today" statements from 2026-06-04 are historical
routing notes and must be verified against live code before use.

## 1. Original Recon Context (Historical)

### Entry path

```
tree_shake_luau()
→ lower all non-extern functions to one TirModule
→ for each local TIR function:
    → type_refine::refine_types()
    → run_pipeline(&mut tir_func, &TargetInfo::luau_release_fast())
    → type_refine::refine_types()
→ run_module_pipeline(&mut module, &TargetInfo::luau_release_fast(), empty_non_inlinable)
→ fail-closed lower_to_simple_ir() for each module function
→ eliminate_dead_ops()
→ LuauBackend::compile_checked(&ir)
```

Current key fact: Luau has an explicit `TargetInfo::luau_release_fast()` path and
runs the shared TIR module phase before checked text emission. `CheckedAdd`
therefore reaches Luau as SimpleIR `checked_add`; the backend consumes it through
the f64 helper contract instead of target-gating the portable TIR transform.

### SimpleIR op dispatch in `emit_op` (luau.rs:1209–4592)

`emit_op` is a `match op.kind.as_str()` covering every known SimpleIR op kind as string literals. The default arm at `luau.rs:4580–4590` emits `local {out} = nil -- [unsupported op: {kind}]`, which survives text emission but is caught by `compile_checked` → `validate_luau_source`, which rejects outputs containing `[unsupported op:` markers. This is the fail-closed gate.

### Integer arithmetic lowering in the Luau backend

All Python integer arithmetic (`add`, `sub`, `mul`, etc.) is handled at `luau.rs:1447–1569`. The `"add"` arm (`luau.rs:1447`) checks `scalar_plan.op_prefers_integer_runtime_lane(op)` — when numeric, emits `local {out}: number = {lhs} + {rhs}` as direct Luau f64 arithmetic.

**Luau's number model:** All numbers are IEEE 754 f64. There are no i64 integers. `math.floor`, `//`, and `%` simulate integer semantics within the 53-bit mantissa range (2^53). Python integers with exact values up to 2^53 are represented exactly; values between 2^53 and 2^63-1 lose precision (wrong digits in lower bits); values ≥ 2^63 are inf or lose all integer meaning. The 2^47 NaN-box inline limit of native/WASM is irrelevant for Luau — Luau uses f64 natively throughout.

**Consequence for overflow detection:** Signed i64 overflow has no meaning in Luau's f64 world. A Python integer that overflows i64 (wraps past 2^63-1) is representable in f64 as `9.223372036854776e18` — a rounded, inexact value — with no hardware overflow signal. Cranelift's `sadd_overflow` detects the 64-bit boundary (2^63); Luau's `+` silently floats past it. The overflow detection mechanism that makes `CheckedAdd` work on native/WASM/LLVM does not exist in Luau.

## 2. Multi-Result Op Precedent: `iter_next_unboxed`

`OpCode::IterNextUnboxed` is the only multi-result TIR op that currently reaches all backends, including Luau. Its lowering is the canonical precedent.

**TIR representation** (`tir/ops.rs:112–115`): `results[0]` = value, `results[1]` = done_flag.

**`lower_to_simple.rs:1620–1631`:**
```rust
OpCode::IterNextUnboxed => {
    let val_var = op.results.first().map(|v| value_var(*v));
    let done_var = op.results.get(1).map(|v| value_var(*v));
    Some(OpIR {
        kind: "iter_next_unboxed".to_string(),
        args: Some(operand_args(op)),
        out: done_var,       // results[1] → op.out
        var: val_var,        // results[0] → op.var
        ..OpIR::default()
    })
}
```

The two outputs are packed into the single-output `OpIR` using `out` (for one result) and `var` (for the second). No new `OpIR` fields are needed; this is the established contract.

**Luau emission** (`luau.rs:4391–4410`):
```rust
"iter_next_unboxed" => {
    // Call iterator; materialize tmp table; unpack into done+value locals.
    let tmp = format!("__next_{tmp_seed}");
    self.emit_line(&format!("local {tmp} = {iter_var}()"));
    if let Some(done) = done_out {
        self.emit_line(&format!("local {done} = {tmp}[2]"));
    }
    if let Some(value) = value_out {
        self.emit_line(&format!("local {value} = {tmp}[1]"));
    }
}
```

The pattern: emit a helper call, then extract each result into a separate `local`. This is the exact template for a Luau runtime helper approach.

## 3. Target-Gating: Does It Exist Today?

No TIR pass queries `tti.target`. There is no mechanism by which a pass in `build_default_pipeline` skips itself for Luau. The only existing examples of pass-level gating are:
- `has_exception_handlers()` inside individual passes (runtime function property, not target)
- `loop_roles.is_empty()` (same)

There is no `TargetInfo::is_luau()` query, no `target == TargetKind::Luau` branch anywhere in the pass infrastructure or pass bodies.

## 4. The Core Problem for CheckedAdd on Luau

`OpCode::CheckedAdd` semantics: `(sum, overflow_flag) = checked_i64_signed_add(lhs, rhs)` where `overflow_flag` is 1 iff the addition overflowed signed 64-bit range. The transform's fast loop uses `overflow_flag` to branch to the slow BigInt path.

On Luau:
- There is no i64 type — all numbers are f64.
- There is no signed 64-bit overflow signal from `+`.
- The threshold that matters is 2^53 (f64 mantissa precision loss), NOT 2^63 (i64 wrap). A loop accumulating past 2^53 in Luau already produces inexact results — but this is ALREADY true of any `add` op emitting bare `+` for a large accumulator. The `overflow_peel` transform does not introduce a new correctness problem here; the Luau backend's number-model already constrains all integer arithmetic to 53-bit precision regardless.
- What `overflow_peel` adds is a `CheckedAdd` op that MUST detect overflow. Emitting it as `lhs + rhs` would be semantically correct for the sum value (same as bare `add`) but would produce a `overflow_flag` that is always false (since f64 `+` never overflows in the Luau sense). The overflow branch to the slow loop would NEVER fire. The fast loop would run to completion with a potentially inexact f64 result — exactly the same as what the un-peeled loop would produce. This is not a correctness regression relative to the current Luau baseline; it is the same behavior the `"add"` op already produces.

## 5. Recommendation: Luau Runtime Helper Approach (Option A)

**Recommendation: emit `CheckedAdd` as a Luau runtime helper call.**

The specific helper: `molt_checked_i64_add(lhs, rhs)` returning `(sum, overflow_flag)`.

**Soundness argument:**

In Luau, the correct semantic is: `sum = lhs + rhs` (f64 addition, same as `"add"` today), `overflow_flag = false` always (since f64 never overflows — it rounds instead). The helper:

```luau
@native
local function molt_checked_i64_add(a: number, b: number): (number, boolean)
    return a + b, false
end
```

This is exactly correct under Luau's f64 semantics. The slow-loop overflow path never fires, which means the Luau backend takes only the fast-loop path. Since the fast loop in Luau does the same f64 addition that the un-peeled `"add"` op already does, and the slow loop would have done the same (via `molt_add` which is also f64 addition for non-BigInt values), the result is byte-identical to the un-peeled behavior. No precision is lost or gained.

An alternative formulation: do NOT emit `overflow_flag = false` statically; instead detect when `math.abs(a + b) > 9007199254740992` (2^53) to trigger the slow path at the f64 precision boundary. However, this would mean the Luau slow path fires for values >2^53 where the fast path is no longer exact — which is a Luau-specific precision policy. The conservative `false` helper is simpler, correct, and matches what every other Luau integer op already does (silently loses precision above 2^53 without triggering any alternate path). Introducing a Luau-specific alternate precision threshold that conflicts with the i64 semantics the peel was designed around would be more complexity with no benefit.

**Why not target-conditional pass gating (Option B)?**

The "refuse overflow_peel for Luau" option requires adding a `tti.target == TargetKind::Luau` guard inside `overflow_peel::run` (or creating a Luau-specific `TargetInfo` and threading it through `main.rs:2195`). The cost-model approach is structurally correct for PROFITABILITY decisions — but blocking a soundness-critical transform based on target creates a gap: the fast-loop phi gets `RawI64Safe` repr annotation that the Luau backend would see but could not honor (Luau has no i64 lane). This forces either (a) removing the repr annotation for Luau, requiring the repr plan to be target-aware, or (b) emitting the op and falling through to the `[unsupported op]` stub (a `compile_checked` rejection). Neither is sound.

The helper approach is structurally better: `CheckedAdd` is a first-class TIR opcode with a well-defined semantic per target. Native/LLVM/WASM use hardware overflow detection; Luau uses the f64-compatible helper (overflow is always false, result is plain f64 addition). All four targets get a working binary. No target-conditional logic in the pass infrastructure.

**Why not refusing the peel in pass_manager for Luau specifically?**

Because the PLAN doc's `overflow_peel` was explicitly designed as a portable TIR transform. If the transform fires, the result is correct on all targets (Luau's fast loop just never takes the overflow branch). Refusing the peel for Luau means Luau keeps the boxed-path accumulator, which is already the status quo — not a regression, but not the "parity across ALL targets" mandate either. If the accumulator in the Luau fast loop produces an inexact f64 result (>2^53), it is no less exact than the un-peeled loop would produce — both use the same `+` operator. The peel's correctness guarantee for Luau is "same result as the un-peeled Luau path", which is satisfied by the always-false helper.

## 6. Implemented Files And Evidence

| File | Current authority |
|---|---|
| `runtime/molt-passes/src/tir/lower_to_simple.rs` | `OpCode::CheckedAdd` lowers to SimpleIR `checked_add` with `var` as sum and `out` as overflow flag; guarded by `checked_add_two_result_round_trip_survives_relift`. |
| `runtime/molt-backend/src/luau.rs` | `emit_op` handles `checked_add` with Luau multi-return destructuring from `molt_checked_i64_add`; guarded by `test_compile_checked_lowers_checked_add_helper`. |
| `docs/spec/areas/compiler/luau_support_matrix.generated.md` | Generated from `luau.rs`; lists `checked_add` as `implemented-exact`. |

**The `emit_op` arm for `"checked_add"` in `luau.rs`:**
```rust
"checked_add" => {
    let args = op.args.as_deref().unwrap_or(&[]);
    if args.len() >= 2 {
        let lhs = sanitize_ident(&args[0]);
        let rhs = sanitize_ident(&args[1]);
        let flag_out = op.out.as_deref().map(sanitize_ident);
        let sum_out = op.var.as_deref().map(sanitize_ident);
        let tmp_seed = flag_out.as_deref().or(sum_out.as_deref()).unwrap_or("ca");
        let tmp = format!("__ca_{tmp_seed}");
        self.emit_line(&format!("local {tmp}_sum, {tmp}_flag = molt_checked_i64_add({lhs}, {rhs})"));
        if let Some(sum) = sum_out {
            self.emit_line(&format!("local {sum}: number = {tmp}_sum"));
        }
        if let Some(flag) = flag_out {
            self.emit_line(&format!("local {flag}: boolean = {tmp}_flag"));
        }
    }
}
```

Note: Luau supports multiple-return values from function calls and destructuring via `local a, b = f()`, so the emission can be simplified to a direct multi-return assignment — no intermediate tmp table needed (unlike `iter_next_unboxed` which had to unpack a table). This is more idiomatic and avoids the `[2]` field access anti-pattern for a known boolean.

## 7. `matches!`-Oracle Audit for `CheckedAdd`

Per the PLAN doc's warning and the MEMORY.md lesson from import-error parity work, every `matches!`-based oracle that enumerates opcodes must be extended. In `effects.rs`:

- `opcode_may_throw` (`effects.rs:90`): `CheckedAdd` does NOT throw — it is pure i64 arithmetic. Do NOT add it here.
- `opcode_is_side_effecting` (`effects.rs:137`): `CheckedAdd` is NOT side-effecting — it is CSE-safe, movable. Do NOT add it here.
- `op_has_observable_effect_when_dead` (`effects.rs:213`): inherits from the above two. No addition needed.

`CheckedAdd` is `ReadOnly` (no memory, no exceptions, no side effects). This must be the correct omission from both `matches!` oracles — the effect of omission is `opcode_may_throw → false` (correct) and `opcode_is_side_effecting → false` (correct). These are the safe defaults. The `matches!` trap is only dangerous when the NEW opcode SHOULD be in the set but isn't — `CheckedAdd` correctly belongs to NEITHER set.

## 8. Essential Files

- `runtime/molt-backend/src/luau.rs` — `emit_op` dispatch, conditional helper prelude,
  and `test_compile_checked_lowers_checked_add_helper`.
- `runtime/molt-backend/src/main.rs` — Luau lifts source-emission IR through the
  shared TIR module pipeline before checked source emission.
- `runtime/molt-passes/src/tir/lower_to_simple.rs` — exhaustive `OpCode` lowering
  plus `CheckedAdd` two-result round-trip coverage.
- `runtime/molt-ir/src/tir/op_kinds.toml` — canonical generated opcode
  authority; `CheckedAdd` is registered and generated into backend/frontend
  tables.
- `runtime/molt-passes/src/tir/passes/effects.rs` — `CheckedAdd` remains pure,
  non-throwing, and non-side-effecting by deliberate omission from throwing and
  side-effecting oracles.
- `runtime/molt-passes/src/tir/target_info.rs` — Luau has explicit target info;
  avoid target-gating portable TIR transforms when a backend semantic helper can
  preserve the operation contract.
- `runtime/molt-backend/src/ir.rs` — `OpIR.out` + `OpIR.var` are the two-result
  transport fields; no new SimpleIR field is required.
