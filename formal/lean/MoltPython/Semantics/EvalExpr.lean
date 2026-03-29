/-
  MoltPython.Semantics.EvalExpr -- Python expression evaluation.

  Pure, deterministic evaluation of Python expressions in an environment.
  Uses Option for partiality (type errors, missing variables) and fuel
  for recursion termination.

  This is the source-level analog of MoltTIR.Semantics.EvalExpr.
-/
import MoltPython.Env

set_option autoImplicit false

namespace MoltPython

/-- Evaluate a binary arithmetic/bitwise operator on two Python values.
    Handles int+int, int+float, float+float promotion, and string concatenation.
    Bitwise ops on Int are omitted (Lean's Int lacks HAnd/HOr/HXor instances);
    same approach as MoltTIR: fall to catch-all, add when needed. -/
def evalBinOp (op : BinOp) (a b : PyValue) : Option PyValue :=
  match op, a, b with
  -- int * int -> int
  | .add, .intVal x, .intVal y => some (.intVal (x + y))
  | .sub, .intVal x, .intVal y => some (.intVal (x - y))
  | .mul, .intVal x, .intVal y => some (.intVal (x * y))
  | .mod, .intVal x, .intVal y =>
      if y == 0 then none else some (.intVal (x % y))
  | .floorDiv, .intVal x, .intVal y =>
      if y == 0 then none else some (.intVal (x / y))
  | .pow, .intVal x, .intVal y =>
      if y < 0 then none    -- negative exponent yields float in CPython; model as error
      else some (.intVal (x ^ y.toNat))
  -- int * float -> float (promotion)
  | .add, .intVal x, .floatVal y => some (.floatVal (x + y))
  | .sub, .intVal x, .floatVal y => some (.floatVal (x - y))
  | .mul, .intVal x, .floatVal y => some (.floatVal (x * y))
  | .add, .floatVal x, .intVal y => some (.floatVal (x + y))
  | .sub, .floatVal x, .intVal y => some (.floatVal (x - y))
  | .mul, .floatVal x, .intVal y => some (.floatVal (x * y))
  -- float * float -> float
  | .add, .floatVal x, .floatVal y => some (.floatVal (x + y))
  | .sub, .floatVal x, .floatVal y => some (.floatVal (x - y))
  | .mul, .floatVal x, .floatVal y => some (.floatVal (x * y))
  -- string concatenation
  | .add, .strVal x, .strVal y => some (.strVal (x ++ y))
  -- string repetition (str * int)
  | .mul, .strVal s, .intVal n =>
      if n ≤ 0 then some (.strVal "")
      else some (.strVal (String.join (List.replicate n.toNat s)))
  | .mul, .intVal n, .strVal s =>
      if n ≤ 0 then some (.strVal "")
      else some (.strVal (String.join (List.replicate n.toNat s)))
  -- list concatenation
  | .add, .listVal x, .listVal y => some (.listVal (x ++ y))
  -- tuple concatenation
  | .add, .tupleVal x, .tupleVal y => some (.tupleVal (x ++ y))
  -- catch-all: type mismatch or unmodeled op (including bitwise)
  | _, _, _ => none

/-- Evaluate a unary operator. -/
def evalUnaryOp (op : UnaryOp) (a : PyValue) : Option PyValue :=
  match op, a with
  | .neg, .intVal x => some (.intVal (-x))
  | .neg, .floatVal x => some (.floatVal (-x))
  | .not, v => some (.boolVal (!v.truthy))
  | _, _ => none

/-- Evaluate a single comparison operator. -/
def evalCompareOp (op : CompareOp) (a b : PyValue) : Option Bool :=
  match op with
  | .eq => PyValue.pyEq a b
  | .notEq => (PyValue.pyEq a b).map (!·)
  | .lt => PyValue.pyLt a b
  | .gtE => (PyValue.pyLt a b).map (!·)
  | .gt => PyValue.pyLt b a
  | .ltE => (PyValue.pyLt b a).map (!·)
  | .is =>
      match a, b with
      | .noneVal, .noneVal => some true
      | .boolVal x, .boolVal y => some (x == y)
      | _, _ => some false
  | .isNot =>
      match a, b with
      | .noneVal, .noneVal => some false
      | .boolVal x, .boolVal y => some (x != y)
      | _, _ => some true
  | .«in» =>
      match b with
      | .listVal elts => some (elts.any fun e => PyValue.pyEq a e == some true)
      | .tupleVal elts => some (elts.any fun e => PyValue.pyEq a e == some true)
      | _ => none
  | .notIn =>
      match b with
      | .listVal elts => some (elts.all fun e => PyValue.pyEq a e != some true)
      | .tupleVal elts => some (elts.all fun e => PyValue.pyEq a e != some true)
      | _ => none

/-- List subscript with Python-style negative indexing. -/
def listSubscript (elts : List PyValue) (idx : Int) : Option PyValue :=
  let len : Int := elts.length
  let i := if idx < 0 then idx + len else idx
  if 0 ≤ i ∧ i < len then elts[i.toNat]? else none

/-- Dict lookup by key equality. -/
def dictLookup (entries : List (PyValue × PyValue)) (key : PyValue) : Option PyValue :=
  match entries with
  | [] => none
  | (k, v) :: rest =>
      if PyValue.pyEq key k == some true then some v
      else dictLookup rest key

/-- Evaluate a subscript operation. -/
def evalSubscript (container index : PyValue) : Option PyValue :=
  match container, index with
  | .listVal elts, .intVal idx => listSubscript elts idx
  | .tupleVal elts, .intVal idx => listSubscript elts idx
  | .dictVal entries, key => dictLookup entries key
  | _, _ => none

mutual

/-- Evaluate a Python expression. Uses fuel for termination.
    Returns none on type errors, missing variables, or fuel exhaustion. -/
def evalPyExpr : Nat → PyEnv → PyExpr → Option PyValue
  | 0, _, _ => none
  | _ + 1, _, .intLit n => some (.intVal n)
  | _ + 1, _, .floatLit f => some (.floatVal f)
  | _ + 1, _, .boolLit b => some (.boolVal b)
  | _ + 1, _, .strLit s => some (.strVal s)
  | _ + 1, _, .noneLit => some .noneVal
  | _ + 1, env, .name x => env.lookup x
  | fuel + 1, env, .binOp op left right =>
      match evalPyExpr fuel env left, evalPyExpr fuel env right with
      | some va, some vb => evalBinOp op va vb
      | _, _ => none
  | fuel + 1, env, .unaryOp op operand =>
      match evalPyExpr fuel env operand with
      | some va => evalUnaryOp op va
      | none => none
  | fuel + 1, env, .compare left ops comparators =>
      evalCompareChain fuel env left ops comparators
  | fuel + 1, env, .boolOp op values =>
      evalBoolOp fuel env op values
  | fuel + 1, env, .ifExpr test body orElse =>
      match evalPyExpr fuel env test with
      | some vt => if vt.truthy then evalPyExpr fuel env body
                   else evalPyExpr fuel env orElse
      | none => none
  | _ + 1, _, .call _func _args =>
      -- Function call requires statement-level eval (executing the body).
      -- Placeholder: return none. Full call semantics added with statement eval.
      none
  | fuel + 1, env, .subscript value slice =>
      match evalPyExpr fuel env value, evalPyExpr fuel env slice with
      | some vv, some sv => evalSubscript vv sv
      | _, _ => none
  | fuel + 1, env, .listExpr elts =>
      match evalExprList fuel env elts with
      | some vs => some (.listVal vs)
      | none => none
  | fuel + 1, env, .tupleExpr elts =>
      match evalExprList fuel env elts with
      | some vs => some (.tupleVal vs)
      | none => none
  | fuel + 1, env, .dictExpr keys values =>
      match evalExprList fuel env keys, evalExprList fuel env values with
      | some ks, some vs => some (.dictVal (ks.zip vs))
      | _, _ => none

/-- Evaluate a list of expressions. -/
def evalExprList : Nat → PyEnv → List PyExpr → Option (List PyValue)
  | _, _, [] => some []
  | 0, _, _ :: _ => none
  | fuel + 1, env, e :: rest =>
      match evalPyExpr fuel env e, evalExprList fuel env rest with
      | some v, some vs => some (v :: vs)
      | _, _ => none

/-- Evaluate a chained comparison: a op1 b op2 c ... -/
def evalCompareChain : Nat → PyEnv → PyExpr → List CompareOp → List PyExpr → Option PyValue
  | _, _, _, [], _ => some (.boolVal true)
  | _, _, _, _, [] => some (.boolVal true)
  | 0, _, _, _ :: _, _ :: _ => none
  | fuel + 1, env, left, op :: ops, comp :: comps =>
      match evalPyExpr fuel env left, evalPyExpr fuel env comp with
      | some lv, some rv =>
          match evalCompareOp op lv rv with
          | some result =>
              if !result then some (.boolVal false)
              else evalCompareChain fuel env comp ops comps
          | none => none
      | _, _ => none

/-- Evaluate short-circuit boolean operators. -/
def evalBoolOp : Nat → PyEnv → BoolOp → List PyExpr → Option PyValue
  | _, _, .and, [] => some (.boolVal true)
  | _, _, .or, [] => some (.boolVal false)
  | 0, _, _, _ :: _ => none
  | fuel + 1, env, _, [e] => evalPyExpr fuel env e
  | fuel + 1, env, .and, e :: rest =>
      match evalPyExpr fuel env e with
      | some v => if !v.truthy then some v else evalBoolOp fuel env .and rest
      | none => none
  | fuel + 1, env, .or, e :: rest =>
      match evalPyExpr fuel env e with
      | some v => if v.truthy then some v else evalBoolOp fuel env .or rest
      | none => none

end

/-- Top-level expression evaluator with default fuel. -/
def evalPyExprDefault (env : PyEnv) (e : PyExpr) : Option PyValue :=
  evalPyExpr 1000 env e

end MoltPython
