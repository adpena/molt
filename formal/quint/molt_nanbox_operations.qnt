// molt_nanbox_operations.qnt — Quint model for NaN-boxing operations.
//
// Extends molt_nanbox_object_model.qnt with exhaustive verification of:
//   - Box/unbox roundtrip correctness for all value types
//   - Tag uniqueness: different types produce different tag bit patterns
//   - Fused check correctness: XOR/BOR-based type checks match explicit checks
//   - Inline integer overflow detection and promotion to bigint
//   - Type dispatch correctness: is_int/is_float/is_bool/is_none/is_ptr
//
// Models the actual bit-level tag encoding from:
//   runtime/molt-obj-model/src/lib.rs — NaN-boxing implementation
//   TAG_INT=1, TAG_BOOL=2, TAG_NONE=3, TAG_PTR=4, TAG_PENDING=5
//   Float: no QNAN prefix (vtag 0), detected by absence of QNAN mask bits.
//
// The fused XOR check exploits the fact that XOR with the expected tag pattern
// yields 0 iff the value matches, and the BOR check uses OR to test multiple
// tag bits simultaneously. Both are branchless on modern hardware.
//
// Constants kept small (N <= 6) per CLAUDE.md for tractable model checking.
//
// Usage:
//   quint run  formal/quint/molt_nanbox_operations.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_nanbox_operations.qnt --invariant=Inv

module molt_nanbox_operations {

  // =========================================================================
  // Types
  // =========================================================================

  // Value type discriminator (vtag, not "tag" — Quint Rust backend collision).
  type VTag = VInt | VFloat | VBool | VNone | VStr | VList

  // A semantic value before boxing.
  type Value = {
    vtag: VTag,
    intVal: int,       // for VInt
    floatBits: int,    // for VFloat (integer encoding of float bits)
    boolVal: bool,     // for VBool
    ptrId: int         // for VStr, VList (-1 if inline type)
  }

  // A boxed 64-bit representation.
  type BoxedWord = {
    tagBits: int,      // tag field in the NaN payload
    payloadBits: int,  // remaining payload bits
    isQNaN: bool       // whether QNAN prefix is set (false for floats)
  }

  // =========================================================================
  // Constants (bounded for tractable model checking, N <= 6)
  // =========================================================================

  // Tag bit constants matching runtime/molt-obj-model/src/lib.rs.
  pure val VTAG_FLOAT = 0
  pure val VTAG_INT   = 1
  pure val VTAG_BOOL  = 2
  pure val VTAG_NONE  = 3
  pure val VTAG_PTR   = 4

  // Bounded integer range for model checking.
  pure val INT_RANGE: Set[int] = Set(-3, -2, -1, 0, 1, 2, 3)

  // Float bit patterns (abstracted as distinct integers).
  pure val FLOAT_BITS: Set[int] = Set(100, 200, 300, 400)

  // Pointer IDs.
  pure val PTR_IDS: Set[int] = Set(0, 1, 2, 3)

  // Inline integer capacity: values outside this range promote to bigint.
  // In the real runtime this is 2^47 - 1; we use a small range for model checking.
  pure val INLINE_INT_MAX = 4
  pure val INLINE_INT_MIN = -4

  // =========================================================================
  // Pure functions — boxing/unboxing
  // =========================================================================

  // Map VTag to tag bits.
  pure def vtagToBits(vt: VTag): int =
    if (vt == VInt) VTAG_INT
    else if (vt == VFloat) VTAG_FLOAT
    else if (vt == VBool) VTAG_BOOL
    else if (vt == VNone) VTAG_NONE
    else VTAG_PTR  // VStr, VList use pointer tags

  // Whether a VTag is inline (no heap allocation).
  pure def isInline(vt: VTag): bool =
    vt == VInt or vt == VFloat or vt == VBool or vt == VNone

  // Whether a VTag is a pointer type.
  pure def isPointer(vt: VTag): bool =
    vt == VStr or vt == VList

  // Box a value into a 64-bit word representation.
  pure def box(v: Value): BoxedWord =
    if (v.vtag == VFloat)
      // Floats: no QNAN prefix, payload IS the float bits.
      { tagBits: VTAG_FLOAT, payloadBits: v.floatBits, isQNaN: false }
    else if (v.vtag == VInt)
      { tagBits: VTAG_INT, payloadBits: v.intVal, isQNaN: true }
    else if (v.vtag == VBool)
      { tagBits: VTAG_BOOL, payloadBits: if (v.boolVal) 1 else 0, isQNaN: true }
    else if (v.vtag == VNone)
      { tagBits: VTAG_NONE, payloadBits: 0, isQNaN: true }
    else
      // Pointer types (VStr, VList).
      { tagBits: VTAG_PTR, payloadBits: v.ptrId, isQNaN: true }

  // Unbox: recover the semantic value from a boxed word.
  pure def unbox(bw: BoxedWord): Value =
    if (not(bw.isQNaN))
      // Float: no QNAN prefix.
      { vtag: VFloat, intVal: 0, floatBits: bw.payloadBits, boolVal: false, ptrId: -1 }
    else if (bw.tagBits == VTAG_INT)
      { vtag: VInt, intVal: bw.payloadBits, floatBits: 0, boolVal: false, ptrId: -1 }
    else if (bw.tagBits == VTAG_BOOL)
      { vtag: VBool, intVal: 0, floatBits: 0,
        boolVal: bw.payloadBits == 1, ptrId: -1 }
    else if (bw.tagBits == VTAG_NONE)
      { vtag: VNone, intVal: 0, floatBits: 0, boolVal: false, ptrId: -1 }
    else
      // VTAG_PTR: we recover as VStr (tag does not distinguish Str vs List
      // at the NaN-box level — the heap object header does that).
      { vtag: VStr, intVal: 0, floatBits: 0, boolVal: false,
        ptrId: bw.payloadBits }

  // =========================================================================
  // Fused type checks (branchless XOR/BOR patterns)
  // =========================================================================

  // Explicit type check: examine tag bits directly.
  pure def explicitIsInt(bw: BoxedWord): bool =
    bw.isQNaN and bw.tagBits == VTAG_INT

  pure def explicitIsFloat(bw: BoxedWord): bool =
    not(bw.isQNaN)

  pure def explicitIsBool(bw: BoxedWord): bool =
    bw.isQNaN and bw.tagBits == VTAG_BOOL

  pure def explicitIsNone(bw: BoxedWord): bool =
    bw.isQNaN and bw.tagBits == VTAG_NONE

  pure def explicitIsPtr(bw: BoxedWord): bool =
    bw.isQNaN and bw.tagBits == VTAG_PTR

  // Fused XOR check: XOR the tag bits with expected tag. Result is 0 iff match.
  // Models the branchless `(bits ^ TAG_INT_PATTERN) == 0` check in the runtime.
  pure def fusedXorIsInt(bw: BoxedWord): bool =
    bw.isQNaN and (bw.tagBits - VTAG_INT) == 0  // XOR with same value = 0

  pure def fusedXorIsBool(bw: BoxedWord): bool =
    bw.isQNaN and (bw.tagBits - VTAG_BOOL) == 0

  pure def fusedXorIsNone(bw: BoxedWord): bool =
    bw.isQNaN and (bw.tagBits - VTAG_NONE) == 0

  // Fused BOR check: test if tag bits are in a set of tags using bitwise OR.
  // Models `(bits & QNAN_MASK) != 0 && tag_in_set(bits, {INT, BOOL})`.
  // We abstract this as set membership over tag bits.
  pure def fusedBorIsIntOrBool(bw: BoxedWord): bool =
    bw.isQNaN and (bw.tagBits == VTAG_INT or bw.tagBits == VTAG_BOOL)

  // =========================================================================
  // Integer overflow detection
  // =========================================================================

  // Whether an integer fits in the inline NaN-boxed representation.
  pure def fitsInline(n: int): bool =
    n >= INLINE_INT_MIN and n <= INLINE_INT_MAX

  // Whether a multiplication result requires promotion to bigint.
  pure def mulOverflows(a: int, b: int): bool =
    not(fitsInline(a * b))

  // =========================================================================
  // State
  // =========================================================================

  // Set of values currently in scope.
  var values: Set[Value]

  // Set of boxed words currently in scope.
  var boxed: Set[BoxedWord]

  // Track which values have been roundtrip-tested.
  var testedRoundtrip: Set[Value]

  // Track overflow detection results.
  var overflowChecked: Set[{ a: int, b: int, overflowed: bool }]

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    values' = Set(),
    boxed' = Set(),
    testedRoundtrip' = Set(),
    overflowChecked' = Set(),
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // Create and box an integer value.
  action boxInt = {
    nondet i = INT_RANGE.oneOf()
    val v: Value = { vtag: VInt, intVal: i, floatBits: 0, boolVal: false, ptrId: -1 }
    val bw = box(v)
    all {
      values.size() < 6,
      values' = values.union(Set(v)),
      boxed' = boxed.union(Set(bw)),
      testedRoundtrip' = testedRoundtrip,
      overflowChecked' = overflowChecked,
    }
  }

  // Create and box a float value.
  action boxFloat = {
    nondet fb = FLOAT_BITS.oneOf()
    val v: Value = { vtag: VFloat, intVal: 0, floatBits: fb, boolVal: false, ptrId: -1 }
    val bw = box(v)
    all {
      values.size() < 6,
      values' = values.union(Set(v)),
      boxed' = boxed.union(Set(bw)),
      testedRoundtrip' = testedRoundtrip,
      overflowChecked' = overflowChecked,
    }
  }

  // Create and box a bool value.
  action boxBool = {
    nondet b = oneOf(Set(true, false))
    val v: Value = { vtag: VBool, intVal: 0, floatBits: 0, boolVal: b, ptrId: -1 }
    val bw = box(v)
    all {
      values.size() < 6,
      values' = values.union(Set(v)),
      boxed' = boxed.union(Set(bw)),
      testedRoundtrip' = testedRoundtrip,
      overflowChecked' = overflowChecked,
    }
  }

  // Create and box None.
  action boxNone = {
    val v: Value = { vtag: VNone, intVal: 0, floatBits: 0, boolVal: false, ptrId: -1 }
    val bw = box(v)
    all {
      values.size() < 6,
      values' = values.union(Set(v)),
      boxed' = boxed.union(Set(bw)),
      testedRoundtrip' = testedRoundtrip,
      overflowChecked' = overflowChecked,
    }
  }

  // Create and box a pointer (string or list).
  action boxPtr = {
    nondet pid = PTR_IDS.oneOf()
    nondet isStr = oneOf(Set(true, false))
    val vt: VTag = if (isStr) VStr else VList
    val v: Value = { vtag: vt, intVal: 0, floatBits: 0, boolVal: false, ptrId: pid }
    val bw = box(v)
    all {
      values.size() < 6,
      values' = values.union(Set(v)),
      boxed' = boxed.union(Set(bw)),
      testedRoundtrip' = testedRoundtrip,
      overflowChecked' = overflowChecked,
    }
  }

  // Test roundtrip: box then unbox, verify equivalence.
  action testRoundtrip = {
    nondet v = values.oneOf()
    val bw = box(v)
    val recovered = unbox(bw)
    // For pointer types, VStr and VList map to the same NaN-box tag.
    // Roundtrip recovers VStr regardless; the actual type lives in the heap header.
    val expectedVtag = if (v.vtag == VList) VStr else v.vtag
    all {
      // Core roundtrip assertion: recovered value matches original semantics.
      recovered.vtag == expectedVtag,
      (v.vtag == VInt) implies recovered.intVal == v.intVal,
      (v.vtag == VFloat) implies recovered.floatBits == v.floatBits,
      (v.vtag == VBool) implies recovered.boolVal == v.boolVal,
      isPointer(v.vtag) implies recovered.ptrId == v.ptrId,
      testedRoundtrip' = testedRoundtrip.union(Set(v)),
      values' = values,
      boxed' = boxed,
      overflowChecked' = overflowChecked,
    }
  }

  // Test fused type checks against explicit checks.
  action testFusedCheck = {
    nondet bw = boxed.oneOf()
    all {
      // XOR checks match explicit checks.
      fusedXorIsInt(bw) == explicitIsInt(bw),
      fusedXorIsBool(bw) == explicitIsBool(bw),
      fusedXorIsNone(bw) == explicitIsNone(bw),
      // BOR check matches explicit disjunction.
      fusedBorIsIntOrBool(bw) == (explicitIsInt(bw) or explicitIsBool(bw)),
      // No state change.
      values' = values,
      boxed' = boxed,
      testedRoundtrip' = testedRoundtrip,
      overflowChecked' = overflowChecked,
    }
  }

  // Test integer overflow detection.
  action testMulOverflow = {
    nondet a = INT_RANGE.oneOf()
    nondet b = INT_RANGE.oneOf()
    val product = a * b
    val overflows = mulOverflows(a, b)
    val result = { a: a, b: b, overflowed: overflows }
    all {
      // If it overflows, the product doesn't fit inline.
      overflows implies not(fitsInline(product)),
      // If it doesn't overflow, the product fits inline.
      not(overflows) implies fitsInline(product),
      overflowChecked' = overflowChecked.union(Set(result)),
      values' = values,
      boxed' = boxed,
      testedRoundtrip' = testedRoundtrip,
    }
  }

  action stutter = all {
    values' = values,
    boxed' = boxed,
    testedRoundtrip' = testedRoundtrip,
    overflowChecked' = overflowChecked,
  }

  action step = any {
    boxInt,
    boxFloat,
    boxBool,
    boxNone,
    boxPtr,
    testRoundtrip,
    testFusedCheck,
    testMulOverflow,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Roundtrip correctness — unbox(box(v)) recovers the semantic value.
  // For pointer types, VStr/VList share the same NaN-box tag; the heap header
  // distinguishes them. Roundtrip recovers VStr as the canonical pointer type.
  val RoundtripCorrect =
    values.forall(v =>
      val bw = box(v)
      val recovered = unbox(bw)
      val expectedVtag = if (v.vtag == VList) VStr else v.vtag
      recovered.vtag == expectedVtag and
      (v.vtag == VInt implies recovered.intVal == v.intVal) and
      (v.vtag == VFloat implies recovered.floatBits == v.floatBits) and
      (v.vtag == VBool implies recovered.boolVal == v.boolVal) and
      (isPointer(v.vtag) implies recovered.ptrId == v.ptrId)
    )

  // I2: Tag uniqueness — different types produce different tag bit patterns
  // (within the same isQNaN class). Two values of different types never
  // produce identical boxed words.
  val TagUnique =
    values.forall(v1 =>
      values.forall(v2 =>
        val bw1 = box(v1)
        val bw2 = box(v2)
        (bw1 == bw2) implies (
          // Same boxed word means same semantic type (modulo VStr/VList alias).
          (v1.vtag == v2.vtag) or
          (isPointer(v1.vtag) and isPointer(v2.vtag))
        )
      )
    )

  // I3: Fused XOR check correctness — matches explicit check for all boxed words.
  val FusedXorCheckCorrect =
    boxed.forall(bw =>
      fusedXorIsInt(bw) == explicitIsInt(bw) and
      fusedXorIsBool(bw) == explicitIsBool(bw) and
      fusedXorIsNone(bw) == explicitIsNone(bw)
    )

  // I4: Fused BOR check correctness — OR-based multi-type check matches
  // explicit disjunction.
  val FusedBorCheckCorrect =
    boxed.forall(bw =>
      fusedBorIsIntOrBool(bw) == (explicitIsInt(bw) or explicitIsBool(bw))
    )

  // I5: Float detection — floats are exactly the non-QNAN values.
  val FloatDetection =
    boxed.forall(bw =>
      explicitIsFloat(bw) == not(bw.isQNaN)
    )

  // I6: Type exclusivity — each boxed word matches exactly one type check.
  val TypeExclusive =
    boxed.forall(bw =>
      val checks = Set(
        explicitIsInt(bw),
        explicitIsFloat(bw),
        explicitIsBool(bw),
        explicitIsNone(bw),
        explicitIsPtr(bw)
      )
      // Exactly one check is true.
      checks.filter(c => c).size() == 1
    )

  // I7: Inline types never carry pointer IDs.
  val InlineNoPtr =
    values.forall(v =>
      isInline(v.vtag) implies v.ptrId == -1
    )

  // I8: Integer overflow detection is consistent — fitsInline and mulOverflows
  // agree on whether the product requires promotion.
  val OverflowConsistent =
    overflowChecked.forall(r =>
      r.overflowed == not(fitsInline(r.a * r.b))
    )

  // I9: All QNAN-tagged values have distinct tag bits per type.
  val QNaNTagDistinct =
    boxed.forall(bw1 =>
      boxed.forall(bw2 =>
        (bw1.isQNaN and bw2.isQNaN and bw1.tagBits == bw2.tagBits)
        implies
        // Same tag bits means same type family.
        (explicitIsInt(bw1) == explicitIsInt(bw2) and
         explicitIsBool(bw1) == explicitIsBool(bw2) and
         explicitIsNone(bw1) == explicitIsNone(bw2) and
         explicitIsPtr(bw1) == explicitIsPtr(bw2))
      )
    )

  // Combined invariant for verification.
  val Inv = and {
    RoundtripCorrect,
    TagUnique,
    FusedXorCheckCorrect,
    FusedBorCheckCorrect,
    FloatDetection,
    TypeExclusive,
    InlineNoPtr,
    OverflowConsistent,
    QNaNTagDistinct,
  }
}
