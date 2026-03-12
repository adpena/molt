/-
  MoltTIR.Backend.WasmSyntax -- WebAssembly target AST for the Molt compiler backend.

  Models the subset of WebAssembly syntax that the Molt backend emits
  (runtime/molt-backend/src/wasm.rs). This is a structured representation
  of the WASM binary module format, used as the target language in the
  translation correctness proofs.

  Key WASM-specific modeling decisions:
  - Stack-based execution (push/pop operand stack), unlike Luau which is expression-based
  - 0-based indexing throughout (locals, globals, functions, memory offsets)
  - Linear memory model with explicit alignment constraints
  - NaN-boxed values are carried as i64 on the operand stack
  - Structured control flow (block/loop/if) with label indices

  References:
  - WebAssembly Core Specification 2.0
  - runtime/molt-backend/src/wasm.rs (Molt WASM codegen)
  - runtime/molt-obj-model/src/lib.rs (NaN-boxed object model)
-/
import MoltTIR.Types

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: WASM value types (WebAssembly spec §2.3.1)
-- ======================================================================

/-- WASM value types per the WebAssembly specification §2.3.1.
    Extends the numeric types from WasmABI.lean with reference types
    needed for the full instruction set model. -/
inductive WasmValType where
  | i32        -- 32-bit integer
  | i64        -- 64-bit integer
  | f32        -- 32-bit IEEE 754 float
  | f64        -- 64-bit IEEE 754 float
  | funcref    -- typed function reference
  | externref  -- external host reference
  deriving DecidableEq, Repr

/-- WASM value types are disjoint (exhaustive pairwise). -/
theorem WasmValType.i32_ne_i64 : WasmValType.i32 ≠ WasmValType.i64 := by decide
theorem WasmValType.i32_ne_f64 : WasmValType.i32 ≠ WasmValType.f64 := by decide
theorem WasmValType.i64_ne_f64 : WasmValType.i64 ≠ WasmValType.f64 := by decide
theorem WasmValType.i64_ne_funcref : WasmValType.i64 ≠ WasmValType.funcref := by decide
theorem WasmValType.i64_ne_externref : WasmValType.i64 ≠ WasmValType.externref := by decide

/-- WASM result type: a sequence of value types (spec §2.3.2). -/
abbrev WasmResultType := List WasmValType

/-- WASM function type: params → results (spec §2.3.3). -/
structure WasmFuncType where
  params  : WasmResultType
  results : WasmResultType
  deriving DecidableEq, Repr

-- ======================================================================
-- Section 2: WASM instructions (WebAssembly spec §2.4)
-- ======================================================================

/-- WASM memory argument for load/store instructions (spec §2.4.7).
    Encodes alignment (as power of 2) and static byte offset. -/
structure WasmMemArg where
  align  : Nat  -- alignment hint (log2 of byte alignment)
  offset : Nat  -- static byte offset added to dynamic address
  deriving DecidableEq, Repr

/-- WASM block type for structured control flow (spec §2.4.5). -/
inductive WasmBlockType where
  | empty                         -- no result
  | valType (ty : WasmValType)    -- single result
  | funcType (idx : Nat)          -- type index (multi-value)
  deriving DecidableEq, Repr

/-- WASM instruction set — the subset emitted by the Molt backend.
    Organized by category following the WebAssembly specification.

    The Molt backend primarily uses i64 instructions (NaN-boxed values
    are all carried as i64), with i32 for memory addresses and control
    flow conditions. -/
inductive WasmInstr where
  -- Control flow (spec §2.4.5)
  | unreachable
  | nop
  | block (bt : WasmBlockType) (body : List WasmInstr)
  | loop (bt : WasmBlockType) (body : List WasmInstr)
  | wasm_if (bt : WasmBlockType) (thenBody : List WasmInstr)
            (elseBody : Option (List WasmInstr))
  | br (labelIdx : Nat)              -- unconditional branch
  | br_if (labelIdx : Nat)           -- conditional branch (pops i32)
  | br_table (labels : List Nat) (default : Nat)
  | wasm_return

  -- Variable access (spec §2.4.3)
  | local_get (idx : Nat)
  | local_set (idx : Nat)
  | local_tee (idx : Nat)            -- set + keep on stack
  | global_get (idx : Nat)
  | global_set (idx : Nat)

  -- Memory load/store (spec §2.4.7)
  | i32_load (mem : WasmMemArg)
  | i64_load (mem : WasmMemArg)
  | f32_load (mem : WasmMemArg)
  | f64_load (mem : WasmMemArg)
  | i32_store (mem : WasmMemArg)
  | i64_store (mem : WasmMemArg)
  | f32_store (mem : WasmMemArg)
  | f64_store (mem : WasmMemArg)
  | i32_load8_u (mem : WasmMemArg)   -- zero-extending byte load
  | i32_store8 (mem : WasmMemArg)    -- byte store
  | memory_size                       -- current memory size in pages
  | memory_grow                       -- grow memory (pops i32 pages)

  -- Constants (spec §2.4.1)
  | i32_const (val : Int)
  | i64_const (val : Int)
  | f32_const (val : Int)             -- modeled as fixed-point
  | f64_const (val : Int)             -- modeled as fixed-point

  -- i32 arithmetic (spec §2.4.1)
  | i32_eqz
  | i32_eq
  | i32_ne
  | i32_lt_s
  | i32_le_s
  | i32_gt_s
  | i32_ge_s
  | i32_add
  | i32_sub
  | i32_mul
  | i32_and
  | i32_or
  | i32_xor
  | i32_shl
  | i32_shr_s

  -- i64 arithmetic (spec §2.4.1) — primary type for NaN-boxed values
  | i64_eqz
  | i64_eq
  | i64_ne
  | i64_lt_s
  | i64_le_s
  | i64_gt_s
  | i64_ge_s
  | i64_add
  | i64_sub
  | i64_mul
  | i64_div_s
  | i64_rem_s
  | i64_and
  | i64_or
  | i64_xor
  | i64_shl
  | i64_shr_s

  -- f64 arithmetic (spec §2.4.1)
  | f64_add
  | f64_sub
  | f64_mul
  | f64_div
  | f64_neg
  | f64_abs

  -- Conversions (spec §2.4.6)
  | i32_wrap_i64                      -- i64 → i32 (truncate)
  | i64_extend_i32_s                  -- i32 → i64 (sign-extend)
  | i64_extend_i32_u                  -- i32 → i64 (zero-extend)
  | f64_convert_i64_s                 -- i64 → f64
  | i64_trunc_f64_s                   -- f64 → i64

  -- Function calls (spec §2.4.8)
  | call (funcIdx : Nat)
  | call_indirect (typeIdx : Nat) (tableIdx : Nat)

  -- Reference types (spec §2.4.9)
  | ref_null (ty : WasmValType)
  | ref_is_null

  -- Parametric (spec §2.4.2)
  | drop
  | select
  deriving Repr

-- ======================================================================
-- Section 3: WASM module structure (WebAssembly spec §2.5)
-- ======================================================================

/-- WASM limits: min and optional max (spec §2.5.1). -/
structure WasmLimits where
  min : Nat
  max : Option Nat
  deriving DecidableEq, Repr

/-- WASM memory type (spec §2.5.5). -/
structure WasmMemType where
  limits : WasmLimits
  deriving DecidableEq, Repr

/-- WASM table type (spec §2.5.4). -/
structure WasmTableType where
  elemType : WasmValType
  limits   : WasmLimits
  deriving DecidableEq, Repr

/-- WASM global type (spec §2.5.6). -/
structure WasmGlobalType where
  valType : WasmValType
  mutable_ : Bool
  deriving DecidableEq, Repr

/-- A WASM local variable declaration (type annotation for a function local). -/
structure WasmLocal where
  count : Nat
  ty    : WasmValType
  deriving DecidableEq, Repr

/-- A WASM function body: local declarations + instruction sequence. -/
structure WasmFuncBody where
  locals : List WasmLocal
  body   : List WasmInstr

/-- A WASM function definition: type index + body. -/
structure WasmFunc where
  typeIdx : Nat
  body    : WasmFuncBody

/-- A WASM global definition: type + initializer expression. -/
structure WasmGlobal where
  ty   : WasmGlobalType
  init : List WasmInstr

/-- A WASM export descriptor. -/
inductive WasmExportDesc where
  | func (idx : Nat)
  | table (idx : Nat)
  | memory (idx : Nat)
  | global (idx : Nat)
  deriving DecidableEq, Repr

/-- A WASM export entry. -/
structure WasmExport where
  name : String
  desc : WasmExportDesc
  deriving Repr

/-- A WASM import descriptor. -/
inductive WasmImportDesc where
  | func (typeIdx : Nat)
  | table (ty : WasmTableType)
  | memory (ty : WasmMemType)
  | global (ty : WasmGlobalType)
  deriving Repr

/-- A WASM import entry. -/
structure WasmImport where
  module_ : String
  name    : String
  desc    : WasmImportDesc
  deriving Repr

/-- A complete WASM module as emitted by the Molt backend.
    Corresponds to the full output of the WASM codegen in wasm.rs. -/
structure WasmModule where
  types    : List WasmFuncType
  imports  : List WasmImport
  funcs    : List WasmFunc
  tables   : List WasmTableType
  memories : List WasmMemType
  globals  : List WasmGlobal
  exports  : List WasmExport
  start    : Option Nat          -- optional start function index

-- ======================================================================
-- Section 4: Linear memory model
-- ======================================================================

/-- WASM page size: 64 KiB. -/
def WASM_PAGE_SIZE : Nat := 65536

/-- WASM32 maximum addressable memory: 2^32 bytes (4 GiB). -/
def WASM32_MAX_ADDR : Nat := 2 ^ 32

/-- A memory address is within WASM32 bounds. -/
def addrInBounds (addr : Nat) (size : Nat) (memSize : Nat) : Prop :=
  addr + size ≤ memSize

/-- Natural alignment for a value type (log2 of byte count). -/
def naturalAlignment : WasmValType → Nat
  | .i32 => 2       -- 4-byte aligned
  | .i64 => 3       -- 8-byte aligned
  | .f32 => 2       -- 4-byte aligned
  | .f64 => 3       -- 8-byte aligned
  | .funcref => 2   -- pointer-sized
  | .externref => 2 -- pointer-sized

/-- Byte size of a WASM value type. -/
def valTypeSize : WasmValType → Nat
  | .i32 => 4
  | .i64 => 8
  | .f32 => 4
  | .f64 => 8
  | .funcref => 4
  | .externref => 4

/-- A memory access is aligned if the address is a multiple of 2^align. -/
def isAligned (addr : Nat) (align : Nat) : Prop :=
  addr % (2 ^ align) = 0

-- ======================================================================
-- Section 5: Utility definitions
-- ======================================================================

/-- Check if a WASM instruction is a simple constant push. -/
def WasmInstr.isConst : WasmInstr → Bool
  | .i32_const _ | .i64_const _ | .f32_const _ | .f64_const _ => true
  | _ => false

/-- Check if a WASM instruction is a control flow instruction. -/
def WasmInstr.isControl : WasmInstr → Bool
  | .block _ _ | .loop _ _ | .wasm_if _ _ _
  | .br _ | .br_if _ | .br_table _ _
  | .wasm_return | .unreachable => true
  | _ => false

/-- The Molt WASM function type: all Molt functions take and return NaN-boxed i64 values. -/
def moltWasmFuncType (nParams : Nat) : WasmFuncType :=
  { params := List.replicate nParams .i64
  , results := [.i64]
  }

/-- Molt uses a single linear memory (index 0) for heap-allocated objects.
    Initial size is 1 page (64 KiB), growable. -/
def moltDefaultMemory : WasmMemType :=
  { limits := { min := 1, max := none } }

end MoltTIR.Backend
