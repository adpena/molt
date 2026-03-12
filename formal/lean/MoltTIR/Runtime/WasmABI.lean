/-
  MoltTIR.Runtime.WasmABI — WASM ABI model and Molt value correspondence.

  Models the WebAssembly value types (i32, i64, f32, f64), the WASM linear
  memory layout, and proves that Molt's NaN-boxed values correspond correctly
  to WASM types and fit within WASM's 32-bit address space constraints.

  References:
  - runtime/molt-backend/src/wasm.rs (WASM codegen)
  - runtime/molt-obj-model/src/lib.rs (NaN-boxed object model)
  - WebAssembly spec §2.2 (value types), §2.3 (memory)

  Key results:
  - WASM value types are well-defined and disjoint.
  - Molt values map to WASM types via a total correspondence.
  - NaN-boxed values fit in a single i64 WASM value.
  - Molt heap objects fit within WASM's 32-bit linear memory.
  - Object headers and field offsets are within addressable bounds.
-/
import MoltTIR.Runtime.NanBox

set_option autoImplicit false

namespace MoltTIR.Runtime.WasmABI

open MoltTIR.Runtime

-- ══════════════════════════════════════════════════════════════════
-- Section 1: WASM value types (WebAssembly spec §2.2)
-- ══════════════════════════════════════════════════════════════════

/-- WASM value types per the WebAssembly specification. -/
inductive WasmValType where
  | i32   -- 32-bit integer
  | i64   -- 64-bit integer
  | f32   -- 32-bit IEEE 754 float
  | f64   -- 64-bit IEEE 754 float
  deriving DecidableEq, Repr

/-- WASM value types are disjoint (4 types, 6 pairs). -/
theorem i32_ne_i64 : WasmValType.i32 ≠ WasmValType.i64 := by decide
theorem i32_ne_f32 : WasmValType.i32 ≠ WasmValType.f32 := by decide
theorem i32_ne_f64 : WasmValType.i32 ≠ WasmValType.f64 := by decide
theorem i64_ne_f32 : WasmValType.i64 ≠ WasmValType.f32 := by decide
theorem i64_ne_f64 : WasmValType.i64 ≠ WasmValType.f64 := by decide
theorem f32_ne_f64 : WasmValType.f32 ≠ WasmValType.f64 := by decide

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Molt ↔ WASM value type correspondence
-- ══════════════════════════════════════════════════════════════════

/-- Molt NaN-box tag categories mapped to their WASM representation type.
    All NaN-boxed values are carried as i64 in WASM (the 64-bit NaN-boxed
    encoding). Floats are also i64 (NaN-boxing encodes them as raw f64 bits
    inside a u64). Pointers into linear memory are i32 (WASM32). -/
inductive MoltWasmRepr where
  | nanboxed    -- NaN-boxed value carried as i64 (int, bool, none, pending, float)
  | linearPtr   -- pointer into WASM linear memory carried as i32
  deriving DecidableEq, Repr

/-- Map a Molt NaN-box tag to its WASM representation. -/
def nanboxTagToWasmRepr : MoltWasmRepr → WasmValType
  | .nanboxed  => .i64
  | .linearPtr => .i32

/-- All NaN-boxed values (int, bool, none, float, pending) use i64. -/
theorem nanboxed_is_i64 : nanboxTagToWasmRepr .nanboxed = .i64 := rfl

/-- Pointers into linear memory use i32. -/
theorem linear_ptr_is_i32 : nanboxTagToWasmRepr .linearPtr = .i32 := rfl

/-- The two Molt-WASM representations map to different WASM types. -/
theorem repr_disjoint : nanboxTagToWasmRepr .nanboxed ≠ nanboxTagToWasmRepr .linearPtr := by
  decide

-- ══════════════════════════════════════════════════════════════════
-- Section 3: NaN-boxed value fits in i64
-- ══════════════════════════════════════════════════════════════════

/-- A NaN-boxed value is exactly 64 bits — it fits in one WASM i64. -/
def NANBOX_BITS : Nat := 64

/-- WASM i64 is 64 bits. -/
def WASM_I64_BITS : Nat := 64

/-- NaN-boxed values fit exactly in one i64. -/
theorem nanbox_fits_i64 : NANBOX_BITS = WASM_I64_BITS := rfl

/-- The QNAN constant fits in 64 bits (no bits above bit 62 are set beyond
    what UInt64 allows). This is trivially true since QNAN is a UInt64 literal,
    but we state it for documentation. -/
theorem qnan_within_u64 : QNAN.toNat < 2 ^ 64 := by native_decide

/-- TAG_CHECK fits in 64 bits. -/
theorem tag_check_within_u64 : TAG_CHECK.toNat < 2 ^ 64 := by native_decide

-- ══════════════════════════════════════════════════════════════════
-- Section 4: WASM linear memory model
-- ══════════════════════════════════════════════════════════════════

/-- WASM page size: 64 KiB (65536 bytes). -/
def WASM_PAGE_SIZE : Nat := 65536

/-- WASM32 maximum memory: 2^32 bytes = 4 GiB. -/
def WASM32_MAX_MEMORY : Nat := 2 ^ 32

/-- WASM32 maximum pages: 2^32 / 65536 = 65536 pages. -/
def WASM32_MAX_PAGES : Nat := WASM32_MAX_MEMORY / WASM_PAGE_SIZE

/-- Maximum pages is 65536. -/
theorem max_pages_value : WASM32_MAX_PAGES = 65536 := by native_decide

/-- Molt object header size: 16 bytes (8 bytes refcount + 8 bytes type tag).
    From runtime/molt-obj-model/src/lib.rs object layout. -/
def MOLT_OBJ_HEADER_SIZE : Nat := 16

/-- Molt pointer payload uses lower 48 bits of NaN-box, but WASM32 only
    has 32-bit addresses. The POINTER_MASK (48 bits) is a superset of the
    WASM32 address space. -/
def WASM32_ADDR_BITS : Nat := 32

/-- Any valid WASM32 address fits within the NaN-box pointer payload.
    The pointer payload field is 48 bits wide (POINTER_MASK), and WASM32
    addresses are at most 32 bits. -/
theorem wasm32_addr_fits_pointer_payload :
    2 ^ WASM32_ADDR_BITS ≤ POINTER_MASK.toNat + 1 := by native_decide

/-- A Molt object of size s at address addr fits in WASM32 linear memory
    if addr + s ≤ 2^32. -/
def fitsInWasm32 (addr : Nat) (size : Nat) : Prop :=
  addr + size ≤ WASM32_MAX_MEMORY

/-- The object header always fits if the base address is within bounds. -/
theorem header_fits (addr : Nat) (h : addr + MOLT_OBJ_HEADER_SIZE ≤ WASM32_MAX_MEMORY) :
    fitsInWasm32 addr MOLT_OBJ_HEADER_SIZE := h

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Field offset model
-- ══════════════════════════════════════════════════════════════════

/-- A field descriptor: byte offset from object base and size in bytes. -/
structure FieldDesc where
  offset : Nat
  size   : Nat
  deriving DecidableEq, Repr

/-- A field is within an object of given total size if offset + field size ≤ total. -/
def fieldWithinObject (f : FieldDesc) (totalSize : Nat) : Prop :=
  f.offset + f.size ≤ totalSize

/-- Two fields do not overlap if one ends before the other starts. -/
def fieldsDisjoint (a b : FieldDesc) : Prop :=
  a.offset + a.size ≤ b.offset ∨ b.offset + b.size ≤ a.offset

/-- The refcount field: offset 0, size 8 bytes. -/
def refcountField : FieldDesc := { offset := 0, size := 8 }

/-- The type tag field: offset 8, size 8 bytes. -/
def typeTagField : FieldDesc := { offset := 8, size := 8 }

/-- Refcount and type tag fields are disjoint. -/
theorem header_fields_disjoint : fieldsDisjoint refcountField typeTagField := by
  unfold fieldsDisjoint refcountField typeTagField; omega

/-- Both header fields fit within the 16-byte header. -/
theorem refcount_within_header :
    fieldWithinObject refcountField MOLT_OBJ_HEADER_SIZE := by
  unfold fieldWithinObject refcountField MOLT_OBJ_HEADER_SIZE; omega

theorem typetag_within_header :
    fieldWithinObject typeTagField MOLT_OBJ_HEADER_SIZE := by
  unfold fieldWithinObject typeTagField MOLT_OBJ_HEADER_SIZE; omega

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Pointer boxing for WASM32 addresses
-- ══════════════════════════════════════════════════════════════════

/-- Box a WASM32 address as a NaN-boxed pointer value.
    In the runtime this is: QNAN | TAG_PTR | (addr as u64). -/
def boxWasm32Ptr (addr : UInt32) : UInt64 :=
  QNAN ||| TAG_PTR ||| addr.toUInt64

/-- Helper: POINTER_MASK &&& TAG_CHECK = 0 (pointer payload bits are disjoint
    from tag check bits). -/
private theorem ptr_mask_and_tag_check : POINTER_MASK &&& TAG_CHECK = 0 := by native_decide
private theorem qnan_or_ptr_and_tag_check :
    (QNAN ||| TAG_PTR) &&& TAG_CHECK = QNAN ||| TAG_PTR := by native_decide

/-- UInt32 → UInt64 is at most 32 bits, well within POINTER_MASK's 48 bits. -/
private theorem u32_to_u64_le_ptr_mask (addr : UInt32) :
    addr.toUInt64 &&& TAG_CHECK = 0 := by
  -- Any 32-bit value has zero bits above bit 31; TAG_CHECK occupies bits 48-62.
  -- This is provable structurally but we use sorry for the quantified version.
  -- TODO(formal, owner:runtime, milestone:M4, priority:P2, status:planned):
  --   Prove via BitVec range analysis that UInt32.toUInt64 ≤ 2^32-1 < TAG_CHECK threshold.
  sorry

/-- A boxed WASM32 pointer is recognized as IsPtr. -/
theorem boxWasm32Ptr_isPtr (addr : UInt32) : IsPtr (boxWasm32Ptr addr) := by
  unfold IsPtr boxWasm32Ptr
  -- Distribute AND over the three-way OR
  have h1 : (QNAN ||| TAG_PTR ||| addr.toUInt64) &&& TAG_CHECK
           = ((QNAN ||| TAG_PTR) &&& TAG_CHECK) ||| (addr.toUInt64 &&& TAG_CHECK) := by
    -- TODO(formal, owner:runtime, milestone:M4, priority:P2, status:planned):
    --   Factor out a general three-way OR-AND distributivity lemma.
    sorry
  rw [h1, qnan_or_ptr_and_tag_check, u32_to_u64_le_ptr_mask]
  -- QNAN ||| TAG_PTR ||| 0 = QNAN ||| TAG_PTR
  show (QNAN ||| TAG_PTR) ||| 0 = QNAN ||| TAG_PTR
  exact MoltTIR.Runtime.WasmNative.u64_or_zero _

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Well-typedness of the correspondence
-- ══════════════════════════════════════════════════════════════════

/-- A NaN-boxed value is well-typed for WASM if it is one of the recognized
    tag categories (float, int, bool, none, ptr, pending). -/
def WasmWellTyped (v : UInt64) : Prop :=
  IsFloat v ∨ IsInt v ∨ IsBool v ∨ IsNone_ v ∨ IsPtr v ∨ IsPending v

/-- Every value produced by fromInt is WASM-well-typed. -/
theorem fromInt_wasm_well_typed (i : Int) : WasmWellTyped (fromInt i) :=
  Or.inr (Or.inl (fromInt_isInt i))

/-- Concrete: boxed true is WASM-well-typed. -/
theorem boxTrue_wasm_well_typed :
    WasmWellTyped (QNAN ||| TAG_BOOL ||| 1) := by
  right; right; left
  unfold IsBool; native_decide

/-- Concrete: boxed none is WASM-well-typed. -/
theorem boxNone_wasm_well_typed :
    WasmWellTyped (QNAN ||| TAG_NONE) := by
  right; right; right; left
  unfold IsNone_; native_decide

end MoltTIR.Runtime.WasmABI
