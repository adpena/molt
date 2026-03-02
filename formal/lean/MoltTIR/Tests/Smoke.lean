/-
  MoltTIR.Tests.Smoke — compile-time smoke tests for the formalization.

  These are #eval / #check / example statements that verify the
  definitions are well-formed and compute expected results.
-/
import MoltTIR.Semantics.Determinism
import MoltTIR.Passes.ConstFoldCorrect

namespace MoltTIR.Tests

-- Smoke: expression evaluation produces expected results
#eval evalExpr Env.empty (.val (.int 42))
-- Expected: some (Value.int 42)

#eval evalExpr Env.empty (.bin .add (.val (.int 2)) (.val (.int 3)))
-- Expected: some (Value.int 5)

#eval evalExpr Env.empty (.bin .lt (.val (.int 1)) (.val (.int 2)))
-- Expected: some (Value.bool true)

#eval evalExpr Env.empty (.un .neg (.val (.int 7)))
-- Expected: some (Value.int -7)

-- Smoke: new ops
#eval evalExpr Env.empty (.bin .ne (.val (.int 1)) (.val (.int 2)))
-- Expected: some (Value.bool true)

#eval evalExpr Env.empty (.bin .ge (.val (.int 5)) (.val (.int 3)))
-- Expected: some (Value.bool true)

#eval evalExpr Env.empty (.un .abs (.val (.int (-42))))
-- Expected: some (Value.int 42)

-- Smoke: constant folding works
#eval constFoldExpr (.bin .add (.val (.int 2)) (.val (.int 3)))
-- Expected: Expr.val (Value.int 5)

#eval constFoldExpr (.bin .add (.var 0) (.val (.int 3)))
-- Expected: Expr.bin BinOp.add (Expr.var 0) (Expr.val (Value.int 3))

-- Smoke: nested constant folding
#eval constFoldExpr (.bin .mul (.bin .add (.val (.int 2)) (.val (.int 3))) (.val (.int 4)))
-- Expected: Expr.val (Value.int 20)

-- Smoke: a simple straight-line function using blockList
-- func f(): entry=0, block 0: x0 := 2+3; ret x0
private def smokeFunc : Func := {
  entry := 0
  blockList := [
    (0, {
      params := []
      instrs := [{ dst := 0, rhs := .bin .add (.val (.int 2)) (.val (.int 3)) }]
      term := .ret (.var 0)
    })
  ]
}

#eval runFunc smokeFunc 10
-- Expected: some (Outcome.ret (Value.int 5))

-- Smoke: constant-folded version of the same function
private def smokeFuncFolded := constFoldFunc smokeFunc

#eval runFunc smokeFuncFolded 10
-- Expected: some (Outcome.ret (Value.int 5))

-- Smoke: branching function
-- if true then ret 1 else ret 0
private def branchFunc : Func := {
  entry := 0
  blockList := [
    (0, {
      params := []
      instrs := []
      term := .br (.val (.bool true)) 1 [] 2 []
    }),
    (1, {
      params := []
      instrs := []
      term := .ret (.val (.int 1))
    }),
    (2, {
      params := []
      instrs := []
      term := .ret (.val (.int 0))
    })
  ]
}

#eval runFunc branchFunc 10
-- Expected: some (Outcome.ret (Value.int 1))

-- Smoke: block parameters (loop-like pattern)
-- block 0: jump to block 1 with arg 10
-- block 1(n): if n == 0 then ret 42 else jump to block 1 with (n-1)
private def blockParamFunc : Func := {
  entry := 0
  blockList := [
    (0, {
      params := []
      instrs := []
      term := .jmp 1 [.val (.int 3)]
    }),
    (1, {
      params := [0]  -- param n at var 0
      instrs := [
        { dst := 1, rhs := .bin .eq (.var 0) (.val (.int 0)) },   -- v1 = n == 0
        { dst := 2, rhs := .bin .sub (.var 0) (.val (.int 1)) }   -- v2 = n - 1
      ]
      term := .br (.var 1) 2 [] 1 [.var 2]
    }),
    (2, {
      params := []
      instrs := []
      term := .ret (.val (.int 42))
    })
  ]
}

#eval runFunc blockParamFunc 20
-- Expected: some (Outcome.ret (Value.int 42))

-- Type check: the key theorems exist and have the expected types
#check @evalExpr_deterministic
#check @execFunc_deterministic
#check @constFoldExpr_correct
#check @constFoldInstr_correct

end MoltTIR.Tests
