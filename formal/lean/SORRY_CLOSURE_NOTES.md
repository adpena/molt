# Sorry Closure Working Notes

## Lean 4.28 API patterns that work:

### BEq/Decide bridging
- `¬decide (y < 0) = true` (Bool hyp) → `¬(y < 0)` (Prop): use `simp only [decide_eq_true_eq] at h` or `rwa [decide_eq_true_eq] at h`
- Resolve `if y < 0` with Bool hyp: `rw [if_neg (by rwa [decide_eq_true_eq] at h)]`
- Or: `simp [if_neg (show ¬(y < 0) by omega)]`

### Operator case analysis
- After `cases op <;> simp only [evalBinOp, ...] at hpv htv`: hypotheses become concrete
- Simple ops (add/sub/mul): `cases hpv; simp_all [lowerValue]`
- Conditional ops (mod/floorDiv): `split at hpv <;> first | simp_all | (subst ...; simp [lowerValue])`
- Pow: `simp [if_neg (show ¬(y < 0) by omega), lowerValue]`

### UInt64 / BitVec
- `UInt64.mk` → `UInt64.ofBitVec` in Lean 4.28
- `bv_decide` works after unfolding tag constants: `unfold QNAN TAG_INT TAG_MASK TAG_CHECK at *; bv_decide`
- For mixed BitVec/Int: case split on sign, use `Nat.and_two_pow_sub_one_eq_mod`, `omega`

### List API
- `List.enum` → `List.zipIdx` (tuple order swapped: `(elem, idx)` not `(idx, elem)`)
- `List.bind` → `List.flatMap`
- `List.get?` → `[i]?`
- `List.mem_cons_self x xs` → `List.mem_cons_self` (implicit args)
- `List.not_mem_nil x` → `List.not_mem_nil`

## Remaining 7 sorrys:

### 1. lowerEnv_corr (Correct.lean:62)
**Goal:** `(lowerScopes nm pyEnv.scopes Env.empty) n = some tv`
**Has:** `hnm : nm.lookup x = some n`, `hpyenv : pyEnv.lookup x = some v`, `htv : lowerValue v = some tv`
**Strategy:** Induction on `pyEnv.scopes`. For each scope, either the binding is in the scope (lowerScope sets it) or in a later scope (IH + lowerScope doesn't overwrite). Needs NameMap injectivity.

### 2. binOp IH (Correct.lean:164)
**Goal:** Need `evalExpr tirEnv (.bin (lowerBinOp op) la ra) = some tv`
**Has:** `ih` at fuel `f`, `heval : evalPyBinOp op pvl pvr = some pv`, `hleft/hright : lowerExpr = some la/ra`, `hevl/hevr : evalPyExpr f = some pvl/pvr`
**Strategy:** Need `lowerValue pvl = some tvl` to apply `ih` on left. This requires a lemma: if binOp succeeds with lowerable output, inputs are lowerable.

### 3. unaryOp IH (Correct.lean:180)
**Same as binOp but for unary ops.**

### 4. lowering_reflects_eval (Correct.lean:184)
**Backward direction.** Needs fuel witness construction.

### 5. LuauCorrect string mul (LuauCorrect.lean:294)
**Goal:** String repetition `if n ≤ 0` branch correspondence.
**Issue:** The `all_goals first | ...` chain can't handle it. Needs per-case proof restructure.

### 6. SCCPCorrect var-case (SCCPCorrect.lean:76)
**Goal:** `ρ x = some cv` from `AbsEnvSound σ ρ` and `σ x = .known cv`.
**Strategy:** Migrate all callers to `AbsEnvStrongSound`. Affects 8+ files.

### 7. PassSimulation SSA (PassSimulation.lean:416)
**Goal:** All TIR functions are SSA.
**Strategy:** Fundamental — needs SSA construction formalization.
