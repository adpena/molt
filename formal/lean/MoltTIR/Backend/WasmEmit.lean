/-
  MoltTIR.Backend.WasmEmit -- Translation from MoltTIR to WASM target AST.

  Models the core translation logic in runtime/molt-backend/src/wasm.rs:
  - Expression emission (IR Expr → List WasmInstr) — stack-based, pushes result
  - Instruction emission (IR Instr → List WasmInstr) — evaluates RHS, stores to local
  - Block emission (IR Block → WasmBlock fragment)
  - Value correspondence: MoltTIR.Value → WASM stack value (NaN-boxed i64)
  - NaN-boxing encoding in WASM linear memory

  Key difference from LuauEmit: WASM is a stack machine. Each emitExpr call
  produces instructions that, when executed, leave exactly one value on the
  operand stack. Instruction emission pops that value and stores it to a local.

  Unlike Luau's 1-based indexing, WASM uses 0-based indexing throughout
  (locals, memory offsets, function indices, table indices).

  References:
  - runtime/molt-backend/src/wasm.rs (Molt WASM codegen)
  - runtime/molt-obj-model/src/lib.rs (NaN-boxed object model)
  - MoltTIR.Runtime.WasmABI (NaN-boxing constants)
  - MoltTIR.Runtime.WasmNative (boxing operations)
-/
import MoltTIR.Syntax
import MoltTIR.Semantics.EvalExpr
import MoltTIR.Backend.WasmSyntax
import MoltTIR.Runtime.WasmNative

set_option autoImplicit false

namespace MoltTIR.Backend

open MoltTIR.Runtime
open MoltTIR.Runtime.WasmNative

-- ======================================================================
-- Section 1: Variable mapping context
-- ======================================================================

/-- Maps IR SSA variable IDs to WASM local indices.
    In the real backend, locals are allocated sequentially: function params
    first, then SSA temps. -/
abbrev WasmLocals := MoltTIR.Var → Nat

/-- Default local mapping: identity (var n → local n). -/
def defaultWasmLocal (x : MoltTIR.Var) : Nat := x

-- ======================================================================
-- Section 2: Value correspondence — MoltTIR.Value → WASM i64 constant
-- ======================================================================

/-- Encode a MoltTIR.Value as the NaN-boxed i64 bit pattern that the WASM
    backend would emit as an i64.const instruction.

    This mirrors the box_int / box_bool / box_none / box_pending operations
    in runtime/molt-backend/src/wasm.rs.

    Float encoding: for the formal model, floats are modeled as Int (fixed-point).
    The real backend uses f64.reinterpret, but for proof purposes we treat
    float-as-int consistently with the IR model. -/
def valueToWasmConst : MoltTIR.Value → Int
  | .int n   => (QNAN ||| TAG_INT ||| (UInt64.ofNat n.toNat &&& POINTER_MASK)).toNat
  | .float f => (QNAN ||| TAG_INT ||| (UInt64.ofNat f.toNat &&& POINTER_MASK)).toNat
  | .bool b  => if b then (QNAN ||| TAG_BOOL ||| 1).toNat else (QNAN ||| TAG_BOOL).toNat
  | .str _   => (QNAN ||| TAG_PTR).toNat  -- string pointer (actual addr filled at link time)
  | .none    => (QNAN ||| TAG_NONE).toNat

/-- Every MoltTIR.Value maps to a well-defined WASM constant. -/
theorem valueToWasmConst_total (v : MoltTIR.Value) : ∃ n : Int, valueToWasmConst v = n := by
  exact ⟨_, rfl⟩

-- ======================================================================
-- Section 3: Operator mapping — IR operators → WASM i64 instructions
-- ======================================================================

/-- Map an IR binary operator to the WASM i64 instruction sequence that
    implements it on NaN-boxed operands.

    For integer arithmetic, the real backend:
    1. Extracts payloads (i64.and with INT_MASK)
    2. Performs the operation
    3. Re-boxes with QNAN | TAG_INT | (result & POINTER_MASK)

    For this model, we emit just the core i64 operation. The full
    extract-operate-rebox sequence is modeled in emitBinOpFull. -/
def emitBinOpCore : MoltTIR.BinOp → WasmInstr
  | .add => .i64_add
  | .sub => .i64_sub
  | .mul => .i64_mul
  | .div => .i64_div_s
  | .floordiv => .i64_div_s
  | .mod => .i64_rem_s
  | .pow => .i64_mul      -- approximation; real backend uses a loop/call
  | .eq => .i64_eq
  | .ne => .i64_ne
  | .lt => .i64_lt_s
  | .le => .i64_le_s
  | .gt => .i64_gt_s
  | .ge => .i64_ge_s
  | .bit_and => .i64_and
  | .bit_or => .i64_or
  | .bit_xor => .i64_xor
  | .lshift => .i64_shl
  | .rshift => .i64_shr_s

/-- Map an IR unary operator to the WASM instruction. -/
def emitUnOpCore : MoltTIR.UnOp → List WasmInstr
  | .neg => [.i64_const (-1), .i64_mul]           -- negate via multiply by -1
  | .not => [.i64_eqz]                            -- boolean not as eqz
  | .abs => [.i64_const (-1), .i64_mul]            -- approximation; real uses branching
  | .invert => [.i64_const (-1), .i64_xor]        -- bitwise invert via XOR with -1

-- ======================================================================
-- Section 4: Expression emission (stack-based)
-- ======================================================================

/-- Emit WASM instructions for an IR expression.
    The resulting instruction sequence, when executed, leaves exactly one
    i64 value on the operand stack.

    This is the WASM analogue of LuauEmit.emitExpr, but targets a stack
    machine instead of an expression-based language. -/
def emitExpr (locals : WasmLocals) : MoltTIR.Expr → List WasmInstr
  | .val v => [.i64_const (valueToWasmConst v)]
  | .var x => [.local_get (locals x)]
  | .bin op a b =>
      emitExpr locals a ++ emitExpr locals b ++ [emitBinOpCore op]
  | .un op a =>
      emitExpr locals a ++ emitUnOpCore op

-- ======================================================================
-- Section 5: Instruction and block emission
-- ======================================================================

/-- Emit WASM instructions for an IR instruction (dst := rhs).
    Evaluates the RHS (pushing result onto stack), then stores to the
    local corresponding to the destination variable. -/
def emitInstr (locals : WasmLocals) (i : MoltTIR.Instr) : List WasmInstr :=
  emitExpr locals i.rhs ++ [.local_set (locals i.dst)]

/-- Emit WASM instructions for a terminator. -/
def emitTerminator (locals : WasmLocals) : MoltTIR.Terminator → List WasmInstr
  | .ret e => emitExpr locals e ++ [.wasm_return]
  | .jmp _ _ => [.br 0]  -- jumps become br to enclosing block
  | .br cond _ _ _ _ =>
      emitExpr locals cond ++ [.br_if 0]

/-- Emit WASM instructions for an IR block.
    Concatenates all instruction emissions followed by the terminator. -/
def emitBlock (locals : WasmLocals) (b : MoltTIR.Block) : List WasmInstr :=
  (b.instrs.map (emitInstr locals) |>.flatten) ++ emitTerminator locals b.term

-- ======================================================================
-- Section 6: Function emission
-- ======================================================================

/-- Emit a WASM function from an IR function.
    All locals are typed as i64 (NaN-boxed values). -/
def emitFunc (locals : WasmLocals) (f : MoltTIR.Func) (_nParams : Nat)
    (nLocals : Nat) : WasmFunc :=
  let entryInstrs := match f.blocks f.entry with
    | some b => emitBlock locals b
    | none => [.unreachable]
  { typeIdx := 0  -- type index filled by module assembly
  , body := {
      locals := if nLocals > 0 then [{ count := nLocals, ty := .i64 }] else []
      body := entryInstrs
    }
  }

-- ======================================================================
-- Section 7: NaN-boxing helpers for linear memory
-- ======================================================================

/-- Emit instructions to box an i64 integer payload as a NaN-boxed value.
    Stack: [payload : i64] → [nanboxed : i64]
    Implements: QNAN | TAG_INT | (payload & POINTER_MASK) -/
def emitBoxInt : List WasmInstr :=
  [ .i64_const (POINTER_MASK.toNat)
  , .i64_and
  , .i64_const ((QNAN ||| TAG_INT).toNat)
  , .i64_or
  ]

/-- Emit instructions to unbox an i64 NaN-boxed integer to its payload.
    Stack: [nanboxed : i64] → [payload : i64]
    Implements: val & INT_MASK -/
def emitUnboxInt : List WasmInstr :=
  [ .i64_const (INT_MASK.toNat)
  , .i64_and
  ]

/-- Emit instructions to check if a NaN-boxed value is an integer.
    Stack: [val : i64] → [isInt : i32]
    Implements: (val & TAG_CHECK) == (QNAN | TAG_INT) -/
def emitIsInt : List WasmInstr :=
  [ .i64_const (TAG_CHECK.toNat)
  , .i64_and
  , .i64_const ((QNAN ||| TAG_INT).toNat)
  , .i64_eq
  ]

/-- Emit instructions to store a NaN-boxed i64 value to linear memory.
    Stack: [addr : i32, val : i64] → []
    Uses natural alignment (8 bytes for i64). -/
def emitStoreNanBoxed : List WasmInstr :=
  [ .i64_store { align := 3, offset := 0 } ]

/-- Emit instructions to load a NaN-boxed i64 value from linear memory.
    Stack: [addr : i32] → [val : i64]
    Uses natural alignment (8 bytes for i64). -/
def emitLoadNanBoxed : List WasmInstr :=
  [ .i64_load { align := 3, offset := 0 } ]

/-- Full integer binary operation with NaN-box extract and rebox.
    Stack: [a : i64, b : i64] → [result : i64]
    Implements: box_int(unbox_int(a) OP unbox_int(b)) -/
def emitBinOpFull (op : MoltTIR.BinOp) (tmpLocal : Nat) : List WasmInstr :=
  -- Save b to tmp, unbox a, restore b, unbox b, operate, rebox
  [ .local_set tmpLocal ]     -- save b
  ++ emitUnboxInt             -- unbox a
  ++ [ .local_get tmpLocal ]  -- restore b
  ++ emitUnboxInt             -- unbox b
  ++ [ emitBinOpCore op ]     -- a_payload OP b_payload
  ++ emitBoxInt               -- rebox result

-- ======================================================================
-- Section 8: Module assembly
-- ======================================================================

/-- Assemble a minimal WASM module for a single Molt function.
    The module exports one function and one linear memory. -/
def assembleModule (func : WasmFunc) (funcName : String) : WasmModule :=
  { types := [moltWasmFuncType 0]
  , imports := []
  , funcs := [func]
  , tables := []
  , memories := [moltDefaultMemory]
  , globals := []
  , exports :=
      [ { name := funcName, desc := .func 0 }
      , { name := "memory", desc := .memory 0 }
      ]
  , start := none
  }

end MoltTIR.Backend
