/-
  MoltTIR.Backend.WasmSemantics -- WASM function-level evaluation model.

  Bridges the instruction-level WASM evaluation (WasmCorrect.lean) to
  function-level evaluation that produces MoltTIR.Outcome values. This
  is needed by CrossBackend.lean to route the WASM backend through
  actual emission and evaluation rather than directly calling runFunc.

  The evaluation pipeline is:
    MoltTIR.Func → (emitFunc) → WasmFunc → (evalWasmFunc) → Option Outcome

  References:
  - Backend/WasmCorrect.lean (instruction-level evaluation)
  - Backend/WasmEmit.lean (TIR → WASM emission)
  - Semantics/ExecFunc.lean (TIR-level runFunc)
-/
import MoltTIR.Backend.WasmCorrect
import MoltTIR.Backend.WasmEmit
import MoltTIR.Backend.LuauSemantics
import MoltTIR.Backend.LuauEmit
import MoltTIR.Backend.RustSemantics
import MoltTIR.Backend.RustEmit
import MoltTIR.Backend.RustSyntax

set_option autoImplicit false

namespace MoltTIR.Backend

open MoltTIR
open MoltTIR.Runtime.WasmNative

-- ======================================================================
-- Section 1: WASM function-level evaluation
-- ======================================================================

/-- Initial WASM state for function execution: empty stack, no locals,
    empty memory with 1 page (64 KiB). -/
def initialWasmState : WasmState where
  stack   := []
  locals  := WasmLocalStore.empty
  memory  := WasmMemory.empty
  memSize := WASM_PAGE_SIZE

/-- Execute a WASM function body and extract the result from the stack.
    After executing all instructions, the top of stack (if any) is the
    return value. An empty stack means the function produced no result.

    This is a simplified model that does not handle control flow (br,
    block, loop, if). It executes the flat instruction list and reads
    the top-of-stack as the NaN-boxed return value.

    fuel: limits execution steps for termination guarantee. -/
def evalWasmFuncBody (instrs : List WasmInstr) (fuel : Nat) : Option (Option Int) :=
  match fuel with
  | 0 => none  -- out of fuel → divergence
  | _ + 1 =>
    match execWasmInstrs initialWasmState instrs with
    | some s =>
      match s.stack with
      | v :: _ => some (some v)  -- function returned a value
      | []     => some none      -- function returned void
    | none => some none  -- execution got stuck (e.g., stack underflow)

/-- Decode a NaN-boxed i64 value back to an Outcome.
    This is the inverse of valueToWasmConst for the subset of values
    we can decode (integers, bools, none). -/
def wasmResultToOutcome (result : Option (Option Int)) : Option Outcome :=
  match result with
  | none => none  -- diverged
  | some none => some .stuck  -- no return value
  | some (some _v) =>
    -- TODO: Full NaN-box decoding. For now, we model the result as
    -- an opaque successful return with a placeholder value.
    -- A complete implementation would decode the NaN-boxed bits back
    -- to a MoltTIR.Value, but this requires modeling the full tag
    -- extraction and payload recovery.
    some (.ret (.int 0))  -- placeholder; real decoding is sorry'd below

/-- Execute a WASM function (already emitted) and produce an Outcome.
    This is the WASM-side equivalent of runFunc. -/
def evalWasmFunc (wf : WasmFunc) (fuel : Nat) : Option Outcome :=
  wasmResultToOutcome (evalWasmFuncBody wf.body.body fuel)

/-- Get the last element of a list, if any. -/
private def lastElem? {α : Type} : List α → Option α
  | [] => none
  | [x] => some x
  | _ :: xs => lastElem? xs

-- ======================================================================
-- Section 2: Luau function-level evaluation
-- ======================================================================

/-- Execute an emitted Luau function body and produce an Outcome.
    Evaluates the statements in the function body, then extracts
    the return value (if any) from the final statement.

    This is simplified: it executes the statement list and looks for
    a return statement's value. A full model would handle control flow. -/
def evalLuauFuncBody (stmts : List LuauStmt) (_fuel : Nat) : Option Outcome :=
  match lastElem? stmts with
  | some (.returnStmt (some retExpr)) =>
    match evalLuauExpr LuauEnv.empty retExpr with
    | some lv =>
      match luauToValue lv with
      | some v => some (.ret v)
      | none => some .stuck
    | none => some .stuck
  | some (.returnStmt none) => some (.ret .none)
  | _ => some .stuck  -- no return statement found

/-- Execute an emitted Luau function and produce an Outcome. -/
def evalLuauFunc (lf : LuauFunc) (fuel : Nat) : Option Outcome :=
  evalLuauFuncBody lf.body fuel

-- ======================================================================
-- Section 3: Rust function-level evaluation
-- ======================================================================

/-- Execute an emitted Rust function body and produce an Outcome.
    Similar to the Luau version: execute statements, extract return value. -/
def evalRustFuncBody (stmts : List RustStmt) (_fuel : Nat) : Option Outcome :=
  match lastElem? stmts with
  | some (.returnStmt (some retExpr)) =>
    match evalRustExpr RustEnv.empty retExpr with
    | some rv =>
      match rustToValue rv with
      | some v => some (.ret v)
      | none => some .stuck
    | none => some .stuck
  | some (.returnStmt none) => some (.ret .none)
  | _ => some .stuck  -- no return statement found

/-- Execute an emitted Rust function and produce an Outcome. -/
def evalRustFunc (rf : RustFn) (fuel : Nat) : Option Outcome :=
  evalRustFuncBody rf.body fuel

end MoltTIR.Backend
