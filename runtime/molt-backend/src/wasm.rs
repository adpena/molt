use crate::{FunctionIR, OpIR, SimpleIR, TrampolineKind, TrampolineSpec};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::iter::ExactSizeIterator;
use std::rc::Rc;
use wasm_encoder::{
    BlockType, Catch, CodeSection, ConstExpr, CustomSection, DataSection, DataSymbolDefinition,
    ElementMode, ElementSection, ElementSegment, Elements, Encode, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, Instruction, LinkingSection,
    MemorySection, MemoryType, Module, RawSection, RefType, SymbolTable, TableSection, TableType,
    TagKind, TagSection, TagType, TypeSection, ValType,
};
use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const QNAN_TAG_MASK_I64: i64 = (QNAN | TAG_MASK) as i64;
const QNAN_TAG_PTR_I64: i64 = (QNAN | TAG_PTR) as i64;
const INT_MASK: u64 = (1 << 47) - 1;
const INT_SHIFT: i64 = 17;
const INT_MIN_INLINE: i64 = -(1 << 46);
const INT_MAX_INLINE: i64 = (1 << 46) - 1;
const HEADER_SIZE_BYTES: i32 = 40;
const HEADER_STATE_OFFSET: i32 = -(HEADER_SIZE_BYTES - 16);
const GEN_CONTROL_SIZE: i32 = 48;
const TASK_KIND_FUTURE: i64 = 0;
const TASK_KIND_GENERATOR: i64 = 1;
const TASK_KIND_COROUTINE: i64 = 2;
const RELOC_TABLE_BASE_DEFAULT: u32 = 4096;
const STATE_REMAP_TABLE_MAX_ENTRIES: usize = 4096;
const STATE_REMAP_TABLE_MAX_SPARSITY: usize = 8;
/// Minimum number of sparse remap entries before we attempt `br_table` dispatch.
const BR_TABLE_MIN_ENTRIES: usize = 5;

// ---------------------------------------------------------------------------
// WASM Exception Handling (WASM_OPTIMIZATION_PLAN.md Section 3.6)
//
// Native WASM exception handling replaces the host-imported exception
// mechanism (exception_push/exception_pending/exception_pop) with the
// standardized WASM exception handling instructions (try_table/throw/catch).
//
// The exception tag carries a single i64 payload: the exception object
// handle.  This matches type index 1 in the static type section:
// (i64) -> ().
//
// Current host-call exception model:
//   try block entry:  call exception_push   (push handler frame)
//   after each call:  call exception_pending (poll for raised exception)
//                     br_if to handler      (branch if pending != 0)
//   try block exit:   call exception_pop    (pop handler frame)
//   raise:            call raise            (set pending + unwind)
//
// Native WASM EH model (target):
//   try block entry:  try_table with catch clause
//   after each call:  (eliminated -- WASM catches automatically)
//   try block exit:   end (implicit)
//   raise:            throw $molt_exception <handle>
//
// Estimated impact: 20-40% speedup for exception-heavy code; 5-10%
// binary size reduction from eliminating exception_pending checks.
//
// Gated by MOLT_WASM_NATIVE_EH=1 environment variable.
// ---------------------------------------------------------------------------

/// Type index for the exception tag payload: (i64) -> ()
/// This is type 1 in the static type section.
const TAG_EXCEPTION_FUNC_TYPE: u32 = 1;

/// Tag index for the molt exception tag (first and only tag in the module).
const TAG_EXCEPTION_INDEX: u32 = 0;

// ---------------------------------------------------------------------------
// Multi-value return type indices (WASM 2.0 multi-value proposal)
//
// These type indices are reserved in the static type section for functions
// that return 2-3 i64 values instead of allocating a tuple on the heap.
// This enables the optimization described in WASM_OPTIMIZATION_PLAN.md §3.1:
// eliminate 1 alloc + N field_get calls per multi-return call site.
//
// Builtins that always return a known-size tuple (e.g. divmod -> 2 values,
// dict items iteration -> 2 values) can be migrated to use these signatures
// once both the host import and call-site lowering are updated.
// ---------------------------------------------------------------------------

/// Type index for multi-value return: (i64, i64) -> (i64, i64)
/// Use case: divmod, dict.popitem(), tuple-2 returns
#[allow(dead_code)]
const MULTI_RETURN_2_TYPE: u32 = 31;

/// Type index for multi-value return: (i64, i64, i64) -> (i64, i64, i64)
/// Use case: 3-element tuple returns
#[allow(dead_code)]
const MULTI_RETURN_3_TYPE: u32 = 32;

/// Type index for multi-value return: (i64) -> (i64, i64)
/// Use case: unary operations that produce a pair
#[allow(dead_code)]
const MULTI_RETURN_UNARY_TO_2_TYPE: u32 = 33;

/// Type index for multi-value return: () -> (i64, i64)
/// Use case: nullary builtins that produce a pair
#[allow(dead_code)]
const MULTI_RETURN_NULLARY_TO_2_TYPE: u32 = 34;

/// First dynamic type index; must equal the count of all statically-defined types.
const STATIC_TYPE_COUNT: u32 = 35;

#[derive(Clone, Copy)]
struct DataSegmentInfo {
    size: u32,
}

#[derive(Clone, Copy)]
struct DataRelocSite {
    func_index: u32,
    offset_in_func: u32,
    segment_index: u32,
}

#[derive(Clone, Copy)]
struct DataSegmentRef {
    offset: u32,
    index: u32,
}

/// Transparent wrapper around `BTreeMap<String, u32>` that records which
/// import names are actually looked up during code emission.  Every
/// `Index<&str>` access inserts the key into a shared `HashSet` so we can
/// compute the set of *unused* imports after compilation finishes.
///
/// The `used` set is behind `Rc<RefCell<…>>` so that clones (needed to
/// work around borrow-checker constraints during `compile_func`) share
/// the same tracking set as the original.
#[derive(Clone)]
struct TrackedImportIds {
    inner: BTreeMap<String, u32>,
    used: Rc<RefCell<HashSet<String>>>,
}

impl TrackedImportIds {
    fn new(inner: BTreeMap<String, u32>) -> Self {
        Self {
            inner,
            used: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    fn insert(&mut self, key: String, value: u32) {
        self.inner.insert(key, value);
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return import names that were registered but never accessed.
    fn unused_names(&self) -> Vec<String> {
        let used = self.used.borrow();
        let mut names: Vec<String> = self
            .inner
            .keys()
            .filter(|k| !used.contains(k.as_str()))
            .cloned()
            .collect();
        names.sort();
        names
    }

    fn get(&self, key: &str) -> Option<&u32> {
        let val = self.inner.get(key);
        if val.is_some() {
            self.used.borrow_mut().insert(key.to_string());
        }
        val
    }

    /// Check existence without marking the import as used.
    fn contains_key(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }
}

impl std::ops::Index<&str> for TrackedImportIds {
    type Output = u32;
    fn index(&self, key: &str) -> &u32 {
        self.used.borrow_mut().insert(key.to_string());
        &self.inner[key]
    }
}

struct CompileFuncContext<'a> {
    func_map: &'a HashMap<String, u32>,
    func_indices: &'a HashMap<String, u32>,
    trampoline_map: &'a HashMap<String, u32>,
    table_base: u32,
    import_ids: &'a TrackedImportIds,
    reloc_enabled: bool,
    /// Functions eligible for multi-value return optimization.
    /// Maps function name -> number of return values (2 or 3).
    multi_return_candidates: &'a HashMap<String, usize>,
    /// Import indices stripped in pure profile mode; calls emit `unreachable`.
    #[allow(dead_code)]
    skipped_import_indices: &'a HashSet<u32>,
}

trait TypeSectionExt {
    fn function<P, R>(&mut self, params: P, results: R)
    where
        P: IntoIterator<Item = ValType>,
        P::IntoIter: ExactSizeIterator,
        R: IntoIterator<Item = ValType>,
        R::IntoIter: ExactSizeIterator;
}

impl TypeSectionExt for TypeSection {
    fn function<P, R>(&mut self, params: P, results: R)
    where
        P: IntoIterator<Item = ValType>,
        P::IntoIter: ExactSizeIterator,
        R: IntoIterator<Item = ValType>,
        R::IntoIter: ExactSizeIterator,
    {
        self.ty().function(params, results);
    }
}

// ---------------------------------------------------------------------------
// Constant folding pass (peephole, pre-emission)
//
// Scans IR ops in forward order, tracking which variables hold known constant
// values.  When an arithmetic op's inputs are all constants (and `fast_int` is
// set), the op is replaced with a `const` op holding the computed result.
// This eliminates redundant unbox-compute-box sequences in the emitted WASM,
// yielding a 3-5% binary size reduction on constant-heavy code.
// ---------------------------------------------------------------------------

fn fold_constants(ops: &mut Vec<OpIR>) {
    // Map from variable name -> known constant integer value (raw, unboxed).
    let mut const_ints: HashMap<String, i64> = HashMap::new();
    // Map from variable name -> known constant boolean value.
    let mut const_bools: HashMap<String, bool> = HashMap::new();

    for op in ops.iter_mut() {
        match op.kind.as_str() {
            "const" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), val);
                }
            }
            "const_bool" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_bools.insert(out.clone(), val != 0);
                }
            }

            // Binary integer arithmetic: add, sub, mul, inplace_add, inplace_sub, inplace_mul
            "add" | "sub" | "mul" | "inplace_add" | "inplace_sub" | "inplace_mul"
                if op.fast_int.unwrap_or(false) =>
            {
                if let Some(ref args) = op.args {
                    if args.len() == 2 {
                        let a_val = const_ints.get(&args[0]).copied();
                        let b_val = const_ints.get(&args[1]).copied();
                        if let (Some(a), Some(b)) = (a_val, b_val) {
                            let result = match op.kind.as_str() {
                                "add" | "inplace_add" => a.wrapping_add(b),
                                "sub" | "inplace_sub" => a.wrapping_sub(b),
                                "mul" | "inplace_mul" => a.wrapping_mul(b),
                                _ => unreachable!(),
                            };
                            op.kind = "const".to_string();
                            op.value = Some(result);
                            op.args = None;
                            op.fast_int = None;
                            if let Some(ref out) = op.out {
                                const_ints.insert(out.clone(), result);
                            }
                            continue;
                        }
                    }
                }
                // Output variable is no longer a known constant.
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Bitwise integer ops: bit_and, bit_or, bit_xor and inplace variants
            "bit_and" | "bit_or" | "bit_xor" | "inplace_bit_and" | "inplace_bit_or"
            | "inplace_bit_xor"
                if op.fast_int.unwrap_or(false) =>
            {
                if let Some(ref args) = op.args {
                    if args.len() == 2 {
                        let a_val = const_ints.get(&args[0]).copied();
                        let b_val = const_ints.get(&args[1]).copied();
                        if let (Some(a), Some(b)) = (a_val, b_val) {
                            let result = match op.kind.as_str() {
                                "bit_and" | "inplace_bit_and" => a & b,
                                "bit_or" | "inplace_bit_or" => a | b,
                                "bit_xor" | "inplace_bit_xor" => a ^ b,
                                _ => unreachable!(),
                            };
                            op.kind = "const".to_string();
                            op.value = Some(result);
                            op.args = None;
                            op.fast_int = None;
                            if let Some(ref out) = op.out {
                                const_ints.insert(out.clone(), result);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Boolean not: `not` on a known bool constant.
            "not" => {
                if let Some(ref args) = op.args {
                    if args.len() == 1 {
                        if let Some(&val) = const_bools.get(&args[0]) {
                            let result = !val;
                            op.kind = "const_bool".to_string();
                            op.value = Some(if result { 1 } else { 0 });
                            op.args = None;
                            if let Some(ref out) = op.out {
                                const_bools.insert(out.clone(), result);
                                const_ints.remove(out);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Control flow boundaries: clear all tracked constants.
            "if" | "else" | "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end"
            | "jump" | "label" | "state_switch" => {
                const_ints.clear();
                const_bools.clear();
            }

            // Any other op that writes an output kills the constant for that variable.
            _ => {
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }
        }
    }
}

fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
}

fn box_float(val: f64) -> i64 {
    val.to_bits() as i64
}

fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

fn box_pending() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for b in func_name
        .as_bytes()
        .iter()
        .chain(lane.as_bytes().iter())
        .copied()
    {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= op_idx as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    let id = (hash & ((1u64 << 46) - 1)).max(1);
    id as i64
}

#[allow(dead_code)]
fn emit_unbox_int_local(func: &mut Function, src_local: u32, dst_local: u32) {
    func.instruction(&Instruction::LocalGet(src_local));
    func.instruction(&Instruction::I64Const(INT_MASK as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const(INT_SHIFT));
    func.instruction(&Instruction::I64Shl);
    func.instruction(&Instruction::I64Const(INT_SHIFT));
    func.instruction(&Instruction::I64ShrS);
    func.instruction(&Instruction::LocalSet(dst_local));
}

/// Cache of WASM local indices holding frequently-used i64 constants.
/// When a function body contains 3+ fast_int operations, these locals are
/// pre-allocated and initialized once at function entry, replacing repeated
/// `i64.const` immediates with cheaper `local.get` instructions.
#[derive(Clone, Copy, Default)]
struct ConstantCache {
    int_shift: Option<u32>,
    int_min: Option<u32>,
    int_max: Option<u32>,
}

impl ConstantCache {
    /// Emit the initialization sequence for all cached constants.
    /// Must be called once, right after the WASM `Function` is created and
    /// before any op emission.
    fn emit_init(&self, func: &mut Function) {
        if let Some(local) = self.int_shift {
            func.instruction(&Instruction::I64Const(INT_SHIFT));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.int_min {
            func.instruction(&Instruction::I64Const(INT_MIN_INLINE));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.int_max {
            func.instruction(&Instruction::I64Const(INT_MAX_INLINE));
            func.instruction(&Instruction::LocalSet(local));
        }
    }
}

/// Trusted unbox: when we *know* the value is a NaN-boxed integer (from IR
/// type information / `fast_int`), we can skip the `AND INT_MASK` step.
/// The left-shift by `INT_SHIFT` (17) already discards the upper QNAN+tag
/// bits, so the mask is redundant.  Saves 2 instructions per operand.
fn emit_unbox_int_local_trusted(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
) {
    func.instruction(&Instruction::LocalGet(src_local));
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64Shl);
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64ShrS);
    func.instruction(&Instruction::LocalSet(dst_local));
}

/// Like [`emit_unbox_int_local_trusted`] but uses `local.tee` instead of
/// `local.set`, leaving the unboxed value on the operand stack.  This
/// eliminates a subsequent `local.get` when the caller needs the value
/// immediately after storing it.
fn emit_unbox_int_local_trusted_tee(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
) {
    func.instruction(&Instruction::LocalGet(src_local));
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64Shl);
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64ShrS);
    func.instruction(&Instruction::LocalTee(dst_local));
}

// ---------------------------------------------------------------------------
// Peephole optimization: known-value unbox/box elimination
//
// When we know at compile time that a WASM local holds a NaN-boxed integer
// whose raw value is `v`, we can replace the 4-instruction unbox sequence
// with a single `i64.const v`, and the 4-instruction box sequence with a
// single `i64.const box_int(v)`.  This eliminates redundant box/unbox
// round-trips that commonly occur when a `const` op feeds into a `fast_int`
// arithmetic op.
// ---------------------------------------------------------------------------

/// Peephole-optimized unbox: if `src_local` has a known raw int value in
/// `known_raw`, emit `i64.const <raw>` + `local.set dst` (2 instructions)
/// instead of the 5-instruction shift-based unbox.  Returns `true` if the
/// optimization fired.
fn emit_unbox_int_local_trusted_opt(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
    known_raw: &HashMap<u32, i64>,
) {
    if let Some(&raw) = known_raw.get(&src_local) {
        func.instruction(&Instruction::I64Const(raw));
        func.instruction(&Instruction::LocalSet(dst_local));
    } else {
        emit_unbox_int_local_trusted(func, src_local, dst_local, cc);
    }
}

/// Peephole-optimized unbox with tee: like [`emit_unbox_int_local_trusted_opt`]
/// but leaves the value on the operand stack (`local.tee`).
fn emit_unbox_int_local_trusted_tee_opt(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
    known_raw: &HashMap<u32, i64>,
) {
    if let Some(&raw) = known_raw.get(&src_local) {
        func.instruction(&Instruction::I64Const(raw));
        func.instruction(&Instruction::LocalTee(dst_local));
    } else {
        emit_unbox_int_local_trusted_tee(func, src_local, dst_local, cc);
    }
}

/// Peephole-optimized box: if `src_local` has a known raw int value in
/// `known_raw`, emit `i64.const <boxed>` (1 instruction) instead of the
/// 4-instruction mask+or boxing sequence.
fn emit_box_int_from_local_opt(func: &mut Function, src_local: u32, known_raw: &HashMap<u32, i64>) {
    if let Some(&raw) = known_raw.get(&src_local) {
        func.instruction(&Instruction::I64Const(box_int(raw)));
    } else {
        emit_box_int_from_local(func, src_local);
    }
}

fn emit_box_int_from_local(func: &mut Function, src_local: u32) {
    func.instruction(&Instruction::LocalGet(src_local));
    func.instruction(&Instruction::I64Const(INT_MASK as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const((QNAN | TAG_INT) as i64));
    func.instruction(&Instruction::I64Or);
}

fn emit_inline_int_range_check(func: &mut Function, val_local: u32, cc: &ConstantCache) {
    func.instruction(&Instruction::LocalGet(val_local));
    if let Some(min_local) = cc.int_min {
        func.instruction(&Instruction::LocalGet(min_local));
    } else {
        func.instruction(&Instruction::I64Const(INT_MIN_INLINE));
    }
    func.instruction(&Instruction::I64GeS);
    func.instruction(&Instruction::LocalGet(val_local));
    if let Some(max_local) = cc.int_max {
        func.instruction(&Instruction::LocalGet(max_local));
    } else {
        func.instruction(&Instruction::I64Const(INT_MAX_INLINE));
    }
    func.instruction(&Instruction::I64LeS);
    func.instruction(&Instruction::I32And);
}

fn emit_box_bool_from_i32(func: &mut Function) {
    func.instruction(&Instruction::I64ExtendI32U);
    func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
    func.instruction(&Instruction::I64Or);
}

fn is_stateful_dispatch_terminator(kind: &str) -> bool {
    matches!(
        kind,
        "state_switch"
            | "state_transition"
            | "state_yield"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "if"
            | "else"
            | "end_if"
            | "loop_start"
            | "loop_index_start"
            | "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break"
            | "loop_continue"
            | "loop_end"
            | "jump"
            | "try_start"
            | "try_end"
            | "label"
            | "state_label"
            | "check_exception"
            | "ret"
            | "ret_void"
    )
}

fn build_dispatch_blocks(ops: &[OpIR]) -> (Vec<usize>, Vec<usize>) {
    let op_count = ops.len();
    if op_count == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut is_start = vec![false; op_count];
    is_start[0] = true;
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "label" | "state_label" | "loop_start" | "loop_index_start" | "loop_end" => {
                is_start[idx] = true;
            }
            _ => {}
        }
        if is_stateful_dispatch_terminator(op.kind.as_str()) && idx + 1 < op_count {
            is_start[idx + 1] = true;
        }
    }

    let mut starts = Vec::new();
    for (idx, start) in is_start.iter().enumerate() {
        if *start {
            starts.push(idx);
        }
    }

    let mut block_for_op = vec![0; op_count];
    let mut block_idx = 0usize;
    let mut next_start = starts.get(1).copied().unwrap_or(op_count);
    for (idx, block_slot) in block_for_op.iter_mut().enumerate().take(op_count) {
        if idx == next_start {
            block_idx += 1;
            next_start = starts.get(block_idx + 1).copied().unwrap_or(op_count);
        }
        *block_slot = block_idx;
    }

    (starts, block_for_op)
}

fn build_dispatch_block_map(block_for_op: &[usize]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(block_for_op.len() * 4);
    for &block_idx in block_for_op {
        bytes.extend_from_slice(&(block_idx as u32).to_le_bytes());
    }
    bytes
}

#[derive(Default)]
struct DispatchControlMaps {
    label_to_index: HashMap<i64, usize>,
    else_for_if: HashMap<usize, usize>,
    end_for_if: HashMap<usize, usize>,
    end_for_else: HashMap<usize, usize>,
    loop_continue_target: HashMap<usize, usize>,
    loop_break_target: HashMap<usize, usize>,
}

fn build_dispatch_control_maps(ops: &[OpIR], include_state_labels: bool) -> DispatchControlMaps {
    struct LoopFrame {
        start_idx: usize,
        break_ops: Vec<usize>,
    }

    let mut maps = DispatchControlMaps::default();
    let mut if_stack: Vec<usize> = Vec::new();
    let mut loop_stack: Vec<LoopFrame> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "label" => {
                if let Some(label_id) = op.value {
                    maps.label_to_index.insert(label_id, idx);
                }
            }
            "state_label" if include_state_labels => {
                if let Some(label_id) = op.value {
                    maps.label_to_index.insert(label_id, idx);
                }
            }
            "if" => if_stack.push(idx),
            "else" => {
                if let Some(if_idx) = if_stack.last().copied() {
                    maps.else_for_if.insert(if_idx, idx);
                }
            }
            "end_if" => {
                if let Some(if_idx) = if_stack.pop() {
                    maps.end_for_if.insert(if_idx, idx);
                    if let Some(else_idx) = maps.else_for_if.get(&if_idx).copied() {
                        maps.end_for_else.insert(else_idx, idx);
                    }
                }
            }
            "loop_start" => {
                loop_stack.push(LoopFrame {
                    start_idx: idx,
                    break_ops: Vec::new(),
                });
            }
            "loop_index_start" => {
                // loop_index_start is always preceded by loop_start,
                // which already pushed a LoopFrame. Update the
                // start_idx to point here (the actual loop body start)
                // instead of pushing a duplicate frame.
                if let Some(frame) = loop_stack.last_mut() {
                    frame.start_idx = idx;
                }
            }
            "loop_continue" => {
                if let Some(frame) = loop_stack.last() {
                    maps.loop_continue_target.insert(idx, frame.start_idx);
                }
            }
            "loop_break_if_true" | "loop_break_if_false" | "loop_break" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_ops.push(idx);
                }
            }
            "loop_end" => {
                if let Some(frame) = loop_stack.pop() {
                    for break_idx in frame.break_ops {
                        maps.loop_break_target.insert(break_idx, idx);
                    }
                }
            }
            _ => {}
        }
    }

    maps
}

fn build_state_resume_maps(ops: &[OpIR]) -> (HashMap<i64, usize>, HashMap<String, i64>) {
    let mut state_map: HashMap<i64, usize> = HashMap::new();
    state_map.insert(0, 0);
    let mut const_ints: HashMap<String, i64> = HashMap::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "state_transition" | "state_yield" | "chan_send_yield" | "chan_recv_yield" => {
                if let Some(state_id) = op.value {
                    state_map.insert(state_id, idx + 1);
                }
            }
            "label" | "state_label" => {
                if let Some(state_id) = op.value {
                    state_map.insert(state_id, idx);
                }
            }
            "const" => {
                if let (Some(out), Some(value)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), value);
                }
            }
            _ => {}
        }
    }

    (state_map, const_ints)
}

fn build_dense_state_remap_table(state_map: &HashMap<i64, usize>) -> Option<Vec<u8>> {
    let mut non_negative_entries: Vec<(usize, i64)> = Vec::new();
    for (&state_id, &target_idx) in state_map {
        if state_id < 0 {
            continue;
        }
        let Ok(state_idx) = usize::try_from(state_id) else {
            return None;
        };
        non_negative_entries.push((state_idx, target_idx as i64));
    }
    if non_negative_entries.is_empty() {
        return None;
    }

    let max_state_idx = non_negative_entries
        .iter()
        .map(|(state_idx, _)| *state_idx)
        .max()?;
    let entry_count = max_state_idx.checked_add(1)?;
    if entry_count > STATE_REMAP_TABLE_MAX_ENTRIES {
        return None;
    }
    if entry_count
        > non_negative_entries
            .len()
            .saturating_mul(STATE_REMAP_TABLE_MAX_SPARSITY)
    {
        return None;
    }

    let mut table = vec![-1i64; entry_count];
    for (state_idx, target_idx) in non_negative_entries {
        table[state_idx] = target_idx;
    }
    let mut bytes = Vec::with_capacity(entry_count * std::mem::size_of::<i64>());
    for target_idx in table {
        bytes.extend_from_slice(&target_idx.to_le_bytes());
    }
    Some(bytes)
}

fn build_sparse_state_remap_entries(state_map: &HashMap<i64, usize>) -> Vec<(i64, i64)> {
    let mut entries = Vec::with_capacity(state_map.len());
    for (&state_id, &target_idx) in state_map {
        if state_id < 0 {
            continue;
        }
        entries.push((state_id, target_idx as i64));
    }
    entries.sort_unstable_by_key(|(state_id, _)| *state_id);
    entries
}

/// Check whether `sorted_entries` form a dense-enough range suitable for
/// `br_table` dispatch.  Returns `Some((min_state, table_size))` when the
/// sparsity ratio (table_size / entry_count) is within
/// `STATE_REMAP_TABLE_MAX_SPARSITY` and there are at least
/// `BR_TABLE_MIN_ENTRIES` entries.
fn br_table_state_remap_params(sorted_entries: &[(i64, i64)]) -> Option<(i64, usize)> {
    if sorted_entries.len() < BR_TABLE_MIN_ENTRIES {
        return None;
    }
    let min_state = sorted_entries.first()?.0;
    let max_state = sorted_entries.last()?.0;
    // table_size covers [min_state, max_state] inclusive.
    let table_size = (max_state - min_state + 1) as usize;
    if table_size
        > sorted_entries
            .len()
            .saturating_mul(STATE_REMAP_TABLE_MAX_SPARSITY)
    {
        return None;
    }
    if table_size > STATE_REMAP_TABLE_MAX_ENTRIES {
        return None;
    }
    Some((min_state, table_size))
}

/// Emit a `br_table`-based O(1) state remap lookup.
///
/// Structure emitted (N = number of remap targets + 1 default):
/// ```wasm
///   block $default          ;; depth 0 – fall-through = no remap
///     block $case_0         ;; depth 1
///       block $case_1       ;; depth 2
///         ...
///       block $case_{N-1}   ;; depth N
///         (local.get state_local)
///         (i64.const min_state)
///         (i64.sub)
///         (i32.wrap_i64)
///         br_table [targets...] $default
///       end  ;; $case_{N-1}
///       ;; set state_local = target for case N-1
///       br $default
///     ...
///   end  ;; $default
/// ```
fn emit_br_table_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
    min_state: i64,
    table_size: usize,
) {
    // Build a mapping from (state_id - min_state) -> target_idx.
    let mut slot_to_target: Vec<Option<i64>> = vec![None; table_size];
    for &(state_id, target_idx) in sorted_entries {
        let slot = (state_id - min_state) as usize;
        slot_to_target[slot] = Some(target_idx);
    }

    // Deduplicate targets to minimise block count: each unique target_idx
    // gets its own block.  Unmapped slots branch to the default (no-op).
    let mut unique_targets: Vec<i64> = sorted_entries.iter().map(|&(_, t)| t).collect();
    unique_targets.sort_unstable();
    unique_targets.dedup();
    let target_block_count = unique_targets.len(); // number of case blocks

    // Map target_idx -> index into unique_targets (0-based).
    let target_to_case: HashMap<i64, usize> = unique_targets
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, i))
        .collect();

    // Block nesting (outermost to innermost):
    //   block $default             depth 0 from br perspective
    //     block $case_0            depth 1
    //       block $case_1          depth 2
    //         ...
    //         block $case_{N-1}    depth N   (= target_block_count)
    //           br_table ...
    //         end $case_{N-1}
    //         <code for case N-1>
    //         br $default          (depth = target_block_count)
    //       end $case_{N-2}
    //       ...
    //     end $case_0
    //   end $default
    //
    // When `br_table` branches to label L, it targets block depth L from
    // the `br_table` instruction.  We want:
    //   - default (unmapped) -> depth 0 ($default, outermost) = skip remap
    //   - case_i             -> depth (target_block_count - i) so that
    //     after `end` of that block we land in code that sets state_local.

    let default_depth: u32 = target_block_count as u32; // reaches $default

    // Build br_table target vector: one entry per table slot.
    let br_targets: Vec<u32> = slot_to_target
        .iter()
        .map(|slot| match slot {
            Some(target_idx) => {
                let case_idx = target_to_case[target_idx];
                // case_idx 0 is outermost case block (depth 1 from br_table).
                // After br_table, we want to land *after* the end of
                // $case_{case_idx}.  The innermost block ($case_0) is at
                // depth target_block_count-1; each subsequent case is one
                // level further out.  So $case_{case_idx} sits at depth
                // (target_block_count - 1 - case_idx).
                (target_block_count - 1 - case_idx) as u32
            }
            None => default_depth,
        })
        .collect();

    // Emit blocks: $default, then $case_0 .. $case_{N-1}.
    func.instruction(&Instruction::Block(BlockType::Empty)); // $default
    for _ in 0..target_block_count {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }

    // Compute table index: (state_local - min_state), then i32.wrap.
    func.instruction(&Instruction::LocalGet(state_local));
    if min_state != 0 {
        func.instruction(&Instruction::I64Const(min_state));
        func.instruction(&Instruction::I64Sub);
    }
    func.instruction(&Instruction::I32WrapI64);

    // br_table dispatch.
    let targets_cow: Cow<[u32]> = br_targets.into();
    func.instruction(&Instruction::BrTable(targets_cow, default_depth));

    // Emit case bodies (innermost block ends first).
    // After `end $case_{N-1-i}`, we're inside $case_{N-2-i}, so we emit
    // the set + branch-to-default for case (N-1-i).
    for rev_i in 0..target_block_count {
        let case_idx = target_block_count - 1 - rev_i;
        func.instruction(&Instruction::End); // end $case_{case_idx}
        let target_idx = unique_targets[case_idx];
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        // Branch to $default to skip remaining cases.
        // Depth from here to $default = case_idx + 1 (because we just
        // closed one block).  Actually, after closing $case_{case_idx},
        // the remaining nesting depth above us is (case_idx) case blocks
        // + 1 default block.  We want to branch to $default which is the
        // outermost, so depth = case_idx.
        if rev_i < target_block_count - 1 {
            func.instruction(&Instruction::Br(case_idx as u32));
        }
        // For the last case (case_idx == 0), we fall through to $default's End.
    }

    func.instruction(&Instruction::End); // end $default
}

fn emit_sparse_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
) {
    // When the entries are dense enough, use br_table for O(1) dispatch.
    if let Some((min_state, table_size)) = br_table_state_remap_params(sorted_entries) {
        emit_br_table_state_remap_lookup(func, state_local, sorted_entries, min_state, table_size);
        return;
    }

    // Fallback: binary-search tree of nested if/else.
    fn emit_node(func: &mut Function, state_local: u32, entries: &[(i64, i64)]) {
        if entries.is_empty() {
            return;
        }

        let mid = entries.len() / 2;
        let (state_id, target_idx) = entries[mid];
        let left = &entries[..mid];
        let right = &entries[mid + 1..];

        func.instruction(&Instruction::LocalGet(state_local));
        func.instruction(&Instruction::I64Const(state_id));
        func.instruction(&Instruction::I64Eq);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        if !left.is_empty() || !right.is_empty() {
            func.instruction(&Instruction::Else);
            match (!left.is_empty(), !right.is_empty()) {
                (true, true) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64LtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, left);
                    func.instruction(&Instruction::Else);
                    emit_node(func, state_local, right);
                    func.instruction(&Instruction::End);
                }
                (true, false) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64LtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, left);
                    func.instruction(&Instruction::End);
                }
                (false, true) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64GtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, right);
                    func.instruction(&Instruction::End);
                }
                (false, false) => {}
            }
        }
        func.instruction(&Instruction::End);
    }

    emit_node(func, state_local, sorted_entries);
}

/// WASM profile for import stripping (see docs/plans/wasm-import-stripping.md §3A).
/// `Full` registers all host imports; `Pure` omits IO, ASYNC, and TIME categories
/// so the resulting module only depends on core runtime + arithmetic + collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmProfile {
    Full,
    Pure,
}

#[derive(Debug, Clone, Copy)]
pub struct WasmCompileOptions {
    pub reloc_enabled: bool,
    pub data_base: u32,
    pub table_base: u32,
    /// Enable native WASM exception handling (WASM 3.0 EH proposal).
    /// Gated by `MOLT_WASM_NATIVE_EH=1` environment variable.
    pub native_eh_enabled: bool,
    /// WASM profile for compile-time import stripping.
    /// Gated by `MOLT_WASM_PROFILE` environment variable ("full" or "pure").
    pub wasm_profile: WasmProfile,
}

impl Default for WasmCompileOptions {
    fn default() -> Self {
        Self {
            reloc_enabled: matches!(std::env::var("MOLT_WASM_LINK").as_deref(), Ok("1")),
            data_base: {
                let raw = std::env::var("MOLT_WASM_DATA_BASE")
                    .ok()
                    .and_then(|val| val.parse::<u64>().ok())
                    .unwrap_or(1_048_576);
                let aligned = (raw + 7) & !7;
                aligned.min(u64::from(u32::MAX)) as u32
            },
            table_base: match std::env::var("MOLT_WASM_TABLE_BASE") {
                Ok(value) => value.parse::<u32>().unwrap_or(RELOC_TABLE_BASE_DEFAULT),
                Err(_) => RELOC_TABLE_BASE_DEFAULT,
            },
            native_eh_enabled: matches!(std::env::var("MOLT_WASM_NATIVE_EH").as_deref(), Ok("1")),
            wasm_profile: match std::env::var("MOLT_WASM_PROFILE").as_deref() {
                Ok("pure") => WasmProfile::Pure,
                _ => WasmProfile::Full,
            },
        }
    }
}

pub struct WasmBackend {
    module: Module,
    types: TypeSection,
    funcs: FunctionSection,
    codes: CodeSection,
    exports: ExportSection,
    imports: ImportSection,
    memories: MemorySection,
    data: DataSection,
    tables: TableSection,
    func_count: u32,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    // Wrapped in TrackedImportIds to record which imports are actually referenced
    // during code emission (see MOLT_WASM_IMPORT_AUDIT).
    import_ids: TrackedImportIds,
    data_offset: u32,
    data_segments: Vec<DataSegmentInfo>,
    data_relocs: Vec<DataRelocSite>,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    data_segment_cache: BTreeMap<Vec<u8>, DataSegmentRef>,
    molt_main_index: Option<u32>,
    options: WasmCompileOptions,
    /// Import indices that were registered but stripped in `pure` profile mode.
    /// Calls to these indices emit `unreachable` instead of `call`.
    #[allow(dead_code)]
    skipped_import_indices: HashSet<u32>,
    /// Number of tail calls emitted via `return_call` (WASM tail calls proposal).
    tail_calls_emitted: usize,
}

impl Default for WasmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmBackend {
    pub fn new() -> Self {
        Self::with_options(WasmCompileOptions::default())
    }

    pub fn with_options(options: WasmCompileOptions) -> Self {
        Self {
            module: Module::new(),
            types: TypeSection::new(),
            funcs: FunctionSection::new(),
            codes: CodeSection::new(),
            exports: ExportSection::new(),
            imports: ImportSection::new(),
            memories: MemorySection::new(),
            data: DataSection::new(),
            tables: TableSection::new(),
            func_count: 0,
            import_ids: TrackedImportIds::new(BTreeMap::new()),
            data_offset: options.data_base,
            data_segments: Vec::new(),
            data_relocs: Vec::new(),
            data_segment_cache: BTreeMap::new(),
            molt_main_index: None,
            options,
            skipped_import_indices: HashSet::new(),
            tail_calls_emitted: 0,
        }
    }

    fn add_data_segment(&mut self, reloc_enabled: bool, bytes: &[u8]) -> DataSegmentRef {
        if let Some(existing) = self.data_segment_cache.get(bytes) {
            return *existing;
        }
        let offset = self.data_offset;
        let index = self.data_segments.len() as u32;
        let const_expr = if reloc_enabled {
            const_expr_i32_const_padded(offset as i32)
        } else {
            ConstExpr::i32_const(offset as i32)
        };
        self.data.active(0, &const_expr, bytes.iter().copied());
        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
        let info = DataSegmentInfo {
            size: bytes.len() as u32,
        };
        self.data_segments.push(info);
        let data_ref = DataSegmentRef { offset, index };
        self.data_segment_cache.insert(bytes.to_vec(), data_ref);
        data_ref
    }

    fn emit_data_ptr(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        data: DataSegmentRef,
    ) {
        let imm_offset = func.byte_len() as u32 + 1;
        self.data_relocs.push(DataRelocSite {
            func_index,
            offset_in_func: imm_offset,
            segment_index: data.index,
        });
        emit_i32_const(func, reloc_enabled, data.offset as i32);
        func.instruction(&Instruction::I64ExtendI32U);
    }

    // ------------------------------------------------------------------
    // Multi-value return analysis  (WASM_OPTIMIZATION_PLAN.md §3.1)
    //
    // Scans every function in the IR and identifies call sites whose
    // result is **immediately destructured** via a fixed number of
    // `tuple_index` ops with constant indices 0..N-1.  These are
    // candidates for the multi-value return optimisation: the callee
    // can push N i64 results directly, and the caller can consume them
    // without a heap-allocated tuple.
    //
    // Returns a map: callee_name -> required_return_count (2 or 3).
    // Only functions where *every* call site destructures to the same
    // arity are included.
    // ------------------------------------------------------------------
    #[allow(dead_code)]
    fn detect_multi_return_candidates(ir: &SimpleIR) -> HashMap<String, usize> {
        // callee -> Option<arity>  (None means conflicting arities => ineligible)
        let mut candidate_arity: HashMap<String, Option<usize>> = HashMap::new();

        for func_ir in &ir.functions {
            let ops = &func_ir.ops;
            for (i, op) in ops.iter().enumerate() {
                // Only consider call_internal (user-defined functions we control).
                if op.kind != "call_internal" {
                    continue;
                }
                let Some(callee) = op.s_value.as_ref() else {
                    continue;
                };
                let Some(result_var) = op.out.as_ref() else {
                    continue;
                };

                // Scan forward to find consecutive tuple_index ops on result_var.
                let mut unpack_count = 0usize;
                let mut seen_indices: HashSet<i64> = HashSet::new();
                for j in (i + 1)..ops.len() {
                    let next_op = &ops[j];
                    if next_op.kind != "tuple_index" {
                        break;
                    }
                    let Some(args) = next_op.args.as_ref() else {
                        break;
                    };
                    if args.len() < 2 || args[0] != *result_var {
                        break;
                    }
                    // The index argument should be a const-int; we check
                    // by looking at the preceding ops, but for this analysis
                    // just count the tuple_index ops.
                    if let Some(idx_val) = next_op.value {
                        seen_indices.insert(idx_val);
                    }
                    unpack_count += 1;
                }

                // Only 2 or 3 element unpacks are worth multi-value.
                // Mark callees with non-destructuring call sites as ineligible.
                if unpack_count < 2 || unpack_count > 3 {
                    candidate_arity.insert(callee.clone(), None);
                    continue;
                }

                // Record or verify consistency.
                match candidate_arity.entry(callee.clone()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(Some(unpack_count));
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if *e.get() != Some(unpack_count) {
                            // Conflicting arities across call sites — not eligible.
                            *e.get_mut() = None;
                        }
                    }
                }
            }
        }

        let call_site_candidates: HashMap<String, usize> = candidate_arity
            .into_iter()
            .filter_map(|(name, arity)| arity.map(|a| (name, a)))
            .collect();

        // Phase 2: Verify the callee function body — every `ret` must return
        // a variable that was produced by a `tuple_new` with the expected arity.
        // This ensures the callee genuinely always returns a fixed-size tuple.
        let func_map: HashMap<&str, &FunctionIR> =
            ir.functions.iter().map(|f| (f.name.as_str(), f)).collect();

        call_site_candidates
            .into_iter()
            .filter(|(name, expected_arity)| {
                let Some(func_ir) = func_map.get(name.as_str()) else {
                    return false;
                };
                // Track which variables are produced by tuple_new of the right arity.
                let mut tuple_new_vars: HashSet<String> = HashSet::new();
                let mut has_any_ret = false;
                let mut all_rets_ok = true;

                for op in &func_ir.ops {
                    match op.kind.as_str() {
                        "tuple_new" => {
                            if let Some(args) = &op.args {
                                if args.len() == *expected_arity {
                                    if let Some(out) = &op.out {
                                        tuple_new_vars.insert(out.clone());
                                    }
                                }
                            }
                        }
                        "ret" => {
                            has_any_ret = true;
                            match &op.var {
                                Some(var) if tuple_new_vars.contains(var) => {}
                                _ => {
                                    all_rets_ok = false;
                                }
                            }
                        }
                        _ => {}
                    }
                }

                has_any_ret && all_rets_ok
            })
            .collect()
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        let mut ir = ir;
        crate::apply_profile_order(&mut ir);
        for func_ir in &mut ir.functions {
            crate::elide_dead_struct_allocs(func_ir);
        }
        for func_ir in &mut ir.functions {
            fold_constants(&mut func_ir.ops);
        }
        crate::inline_functions(&mut ir);

        // Multi-value return candidate detection (§3.1).
        // This analysis identifies internal functions whose call sites always
        // destructure the result via 2-3 consecutive tuple_index ops AND whose
        // body always returns via tuple_new of the matching arity.
        let multi_return_candidates = Self::detect_multi_return_candidates(&ir);

        if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1")
            && !multi_return_candidates.is_empty()
        {
            eprintln!(
                "[molt-wasm-multi-return] {} candidate(s) detected:",
                multi_return_candidates.len()
            );
            let mut sorted: Vec<(&String, &usize)> = multi_return_candidates.iter().collect();
            sorted.sort_by_key(|(name, _)| *name);
            for (name, arity) in &sorted {
                eprintln!("  - {name} (returns {arity} values)");
            }
        }

        // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
        let mut func_trampoline_spec: BTreeMap<String, (usize, bool)> = BTreeMap::new();
        let mut task_kinds: BTreeMap<String, TrampolineKind> = BTreeMap::new();
        let mut task_closure_sizes: BTreeMap<String, i64> = BTreeMap::new();
        for func_ir in &ir.functions {
            let mut func_obj_names: HashMap<String, String> = HashMap::new();
            let mut const_values: HashMap<String, i64> = HashMap::new();
            let mut const_bools: HashMap<String, bool> = HashMap::new();
            let mut pending_attrs: Vec<(String, String, String)> = Vec::new();
            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "const" => {
                        let Some(out) = op.out.as_ref() else {
                            continue;
                        };
                        let val = op.value.unwrap_or(0);
                        const_values.insert(out.clone(), val);
                    }
                    "const_bool" => {
                        let Some(out) = op.out.as_ref() else {
                            continue;
                        };
                        let val = op.value.unwrap_or(0) != 0;
                        const_bools.insert(out.clone(), val);
                    }
                    "func_new" | "func_new_closure" => {
                        let Some(name) = op.s_value.as_ref() else {
                            continue;
                        };
                        let arity = op.value.unwrap_or(0) as usize;
                        let has_closure = op.kind == "func_new_closure";
                        if let Some(out) = op.out.as_ref() {
                            func_obj_names.insert(out.clone(), name.clone());
                        }
                        if let Some((prev_arity, prev_closure)) = func_trampoline_spec.get(name) {
                            if *prev_arity != arity || *prev_closure != has_closure {
                                panic!("func_new arity mismatch for {name}");
                            }
                        } else {
                            func_trampoline_spec.insert(name.clone(), (arity, has_closure));
                        }
                    }
                    "set_attr_generic_obj" => {
                        let Some(attr) = op.s_value.as_deref() else {
                            continue;
                        };
                        if attr != "__molt_is_generator__"
                            && attr != "__molt_is_coroutine__"
                            && attr != "__molt_is_async_generator__"
                            && attr != "__molt_closure_size__"
                        {
                            continue;
                        }
                        let args = op.args.as_ref().expect("set_attr_generic_obj args missing");
                        pending_attrs.push((args[0].clone(), args[1].clone(), attr.to_string()));
                    }
                    _ => {}
                }
            }
            for (func_obj_name, val_name, attr) in pending_attrs {
                let Some(func_name) = func_obj_names.get(&func_obj_name) else {
                    continue;
                };
                match attr.as_str() {
                    "__molt_is_generator__"
                    | "__molt_is_coroutine__"
                    | "__molt_is_async_generator__" => {
                        let is_true = const_bools
                            .get(&val_name)
                            .copied()
                            .or_else(|| const_values.get(&val_name).map(|val| *val != 0))
                            .unwrap_or(false);
                        if is_true {
                            if !func_name.ends_with("_poll") {
                                continue;
                            }
                            let kind = match attr.as_str() {
                                "__molt_is_generator__" => TrampolineKind::Generator,
                                "__molt_is_coroutine__" => TrampolineKind::Coroutine,
                                "__molt_is_async_generator__" => TrampolineKind::AsyncGen,
                                _ => TrampolineKind::Plain,
                            };
                            if let Some(prev) = task_kinds.insert(func_name.clone(), kind)
                                && prev != kind
                            {
                                panic!(
                                    "conflicting task kinds for {func_name}: {:?} vs {:?}",
                                    prev, kind
                                );
                            }
                        }
                    }
                    "__molt_closure_size__" => {
                        if let Some(size) = const_values.get(&val_name) {
                            task_closure_sizes.insert(func_name.clone(), *size);
                        }
                    }
                    _ => {}
                }
            }
        }
        // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
        let mut default_trampoline_spec: BTreeMap<String, (usize, bool)> = BTreeMap::new();
        for func_ir in &ir.functions {
            let default_has_closure = func_ir
                .params
                .first()
                .is_some_and(|name| name == "__molt_closure__");
            let mut default_arity = func_ir.params.len();
            if default_has_closure && default_arity > 0 {
                default_arity = default_arity.saturating_sub(1);
            }
            let spec = func_trampoline_spec
                .get(&func_ir.name)
                .copied()
                .unwrap_or((default_arity, default_has_closure));
            default_trampoline_spec.insert(func_ir.name.clone(), spec);
        }

        // Trampolines now handle multi-value return callees by reconstructing
        // a tuple from the N return values (see compile_trampoline), so we no
        // longer need to exclude trampolined functions from the optimization.
        let multi_return_candidates: HashMap<String, usize> =
            multi_return_candidates.into_iter().collect();

        // Type 0: () -> i64 (User functions)
        self.types
            .function(std::iter::empty::<ValType>(), std::iter::once(ValType::I64));
        // Type 1: (i64) -> () (print_obj)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::empty::<ValType>());
        // Type 2: (i64) -> i64 (alloc, sleep, block_on, is_truthy, is_bound_method)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::once(ValType::I64));
        // Type 3: (i64, i64) -> i64 (add/sub/mul/lt/list_append/list_pop/alloc_class/stream_send_obj)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 2),
            std::iter::once(ValType::I64),
        );
        // Type 4: (i64, i64, i64) -> i32 (parse_scalar)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 3),
            std::iter::once(ValType::I32),
        );
        // Type 5: (i64, i64, i64) -> i64 (stream_send, ws_send, slice, slice_new, dict_get, task_new)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 3),
            std::iter::once(ValType::I64),
        );
        // Type 6: (i64, i64) -> () (list_builder_append)
        self.types
            .function(std::iter::repeat_n(ValType::I64, 2), std::iter::empty());
        // Type 7: (i64, i64, i64, i64) -> i64 (dict_pop)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 4),
            std::iter::once(ValType::I64),
        );
        // Type 8: () -> () (print_newline)
        self.types
            .function(std::iter::empty::<ValType>(), std::iter::empty());
        // Type 9: (i64, i64, i64, i64, i64, i64) -> i64 (string_count_slice)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 6),
            std::iter::once(ValType::I64),
        );
        // Type 10: (i64, i64, i64, i64, i64, i64, i64) -> i64 (guarded_field_set/init)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 7),
            std::iter::once(ValType::I64),
        );
        // Type 11: (i64, i64, i64, i64) -> i32 (db_query/db_exec)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 4),
            std::iter::once(ValType::I32),
        );
        // Type 12: (i64, i64, i64, i64, i64) -> i64 (print_builtin)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 5),
            std::iter::once(ValType::I64),
        );
        // Type 13: (i64) -> i32 (handle_resolve)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::once(ValType::I32));
        // Type 14: (i32) -> i64 (reserved)
        self.types
            .function(std::iter::once(ValType::I32), std::iter::once(ValType::I64));
        // Type 15: (i32) -> () (reserved)
        self.types
            .function(std::iter::once(ValType::I32), std::iter::empty::<ValType>());
        // Type 16: (i32, i64) -> i64 (object_field_get_ptr, closure_load, object_set_class)
        self.types
            .function([ValType::I32, ValType::I64], std::iter::once(ValType::I64));
        // Type 17: (i32, i64, i64) -> i64 (guard_layout_ptr, closure_store, object_field_set/init)
        self.types.function(
            [ValType::I32, ValType::I64, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 18: (i64, i32, i64) -> i64 (stream_send, ws_send, get_attr_object)
        self.types.function(
            [ValType::I64, ValType::I32, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 19: (i32, i64, i32) -> i32 (parse_scalar, ws_connect)
        self.types.function(
            [ValType::I32, ValType::I64, ValType::I32],
            std::iter::once(ValType::I32),
        );
        // Type 20: (i64, i32, i32) -> i32 (ws_pair)
        self.types.function(
            [ValType::I64, ValType::I32, ValType::I32],
            std::iter::once(ValType::I32),
        );
        // Type 21: (i32, i64, i64, i64, i32, i64) -> i64 (guarded_field_get_ptr)
        self.types.function(
            [
                ValType::I32,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I32,
                ValType::I64,
            ],
            std::iter::once(ValType::I64),
        );
        // Type 22: (i32, i64, i64, i64, i64, i32, i64) -> i64 (guarded_field_set/init)
        self.types.function(
            [
                ValType::I32,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I32,
                ValType::I64,
            ],
            std::iter::once(ValType::I64),
        );
        // Type 23: (i32, i32, i64) -> i64 (get/del_attr_generic/ptr)
        self.types.function(
            [ValType::I32, ValType::I32, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 24: (i32, i32, i64, i64) -> i64 (set_attr_generic/ptr)
        self.types.function(
            [ValType::I32, ValType::I32, ValType::I64, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 25: (i64, i32, i64, i64) -> i64 (set_attr_object)
        self.types.function(
            [ValType::I64, ValType::I32, ValType::I64, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 26: (i32, i64, i32, i64) -> i32 (db_query/db_exec)
        self.types.function(
            [ValType::I32, ValType::I64, ValType::I32, ValType::I64],
            std::iter::once(ValType::I32),
        );
        // Type 27: (i32, i32) -> i64 (sleep_register)
        self.types
            .function([ValType::I32, ValType::I32], std::iter::once(ValType::I64));
        // Type 28: (i64, i64, i64, i64, i64, i64, i64, i64) -> i64 (open_builtin, code_new)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 8),
            std::iter::once(ValType::I64),
        );
        // Type 29: (i64, i64, i64, i64, i64, i64) -> i64 (sys_set_version_info)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 6),
            std::iter::once(ValType::I64),
        );
        // Type 30: (i64, i64, i64) -> () (payload init with closure)
        self.types
            .function(std::iter::repeat_n(ValType::I64, 3), std::iter::empty());

        // ---------------------------------------------------------------
        // Multi-value return types (WASM 2.0, universally supported)
        //
        // These signatures enable returning 2-3 i64 values on the WASM
        // operand stack, eliminating heap-allocated tuples for small
        // multi-returns.  See WASM_OPTIMIZATION_PLAN.md §3.1.
        // ---------------------------------------------------------------

        // Type 31 (MULTI_RETURN_2_TYPE): (i64, i64) -> (i64, i64)
        // Target: divmod_builtin, dict.popitem(), enumerate next, etc.
        self.types.function(
            std::iter::repeat_n(ValType::I64, 2),
            std::iter::repeat_n(ValType::I64, 2),
        );

        // Type 32 (MULTI_RETURN_3_TYPE): (i64, i64, i64) -> (i64, i64, i64)
        // Target: 3-element tuple returns
        self.types.function(
            std::iter::repeat_n(ValType::I64, 3),
            std::iter::repeat_n(ValType::I64, 3),
        );

        // Type 33 (MULTI_RETURN_UNARY_TO_2_TYPE): (i64) -> (i64, i64)
        // Target: unary operations that produce a pair
        self.types.function(
            std::iter::once(ValType::I64),
            std::iter::repeat_n(ValType::I64, 2),
        );

        // Type 34 (MULTI_RETURN_NULLARY_TO_2_TYPE): () -> (i64, i64)
        // Target: nullary builtins that produce a pair
        self.types.function(
            std::iter::empty::<ValType>(),
            std::iter::repeat_n(ValType::I64, 2),
        );

        // Build the set of import name prefixes to skip in "pure" profile mode.
        // In pure mode, IO/ASYNC/TIME imports are omitted entirely. Any code path
        // that references a skipped import will trigger a clear compile-time panic.
        let is_pure = self.options.wasm_profile == WasmProfile::Pure;
        let skipped_import_prefixes: &[&str] = if is_pure {
            &[
                // IO
                "process_",
                "socket",
                "os_",
                "db_",
                "ws_",
                "file_",
                "stream_",
                "path_exists",
                "path_listdir",
                "path_mkdir",
                "path_unlink",
                "path_rmdir",
                "path_chmod",
                "open_builtin",
                // ASYNC
                "async_sleep",
                "future_",
                "promise_",
                "thread_",
                "lock_",
                "rlock_",
                "chan_",
                "asyncio_",
                "asyncgen_",
                "anext_",
                "io_wait",
                "spawn",
                "block_on",
                "cancel_token_",
                "cancelled",
                "cancel_current",
                "sleep_register",
                "contextlib_async",
                // TIME
                "time_",
                // COMPRESSION
                "deflate_raw",
                "inflate_raw",
                "bz2_",
                "gzip_",
                "lzma_",
                "zlib_",
                "compression_",
                // SERIALIZATION (msgpack/cbor — JSON stays)
                "msgpack_",
                "cbor_",
                // CRYPTO (hashlib — sha2/sha1/md5 stay as core)
                "hash_new",
                "hash_update",
                "hash_digest",
                "hash_hexdigest",
                "hash_copy",
                "hmac_",
                "pbkdf2_",
                "scrypt",
                "compare_digest",
                "secrets_",
                // AST
                "ast_",
                // ARCHIVE
                "zipfile_",
                // FS EXTRA
                "glob_",
                "tempfile_",
                "tarfile_",
            ]
        } else {
            &[]
        };
        let is_skipped_import = |name: &str| -> bool {
            if !is_pure {
                return false;
            }
            for prefix in skipped_import_prefixes {
                if name.starts_with(prefix) {
                    return true;
                }
            }
            false
        };

        let mut import_idx = 0;
        let skipped_indices: HashSet<u32> = HashSet::new();
        let mut add_import = |name: &str, ty: u32, ids: &mut TrackedImportIds| {
            if is_skipped_import(name) {
                // In pure mode, skip IO/ASYNC/TIME imports entirely.
                // The import is not registered in the WASM module, so the
                // resulting binary has no dependency on these host functions.
                // Insert a sentinel value so that `import_ids["name"]` lookups
                // succeed (no panic), and `emit_call` emits `unreachable`.
                ids.insert(name.to_string(), u32::MAX);
                return;
            }
            self.imports
                .import("molt_runtime", name, EntityType::Function(ty));
            ids.insert(name.to_string(), import_idx);
            import_idx += 1;
        };
        let simple_i64_import_type = |arity: usize| -> u32 {
            match arity {
                0 => 0,
                1 => 2,
                2 => 3,
                3 => 5,
                4 => 7,
                5 => 12,
                6 => 9,
                7 => 10,
                8 => 28,
                _ => panic!("unsupported simple i64 import arity {arity}"),
            }
        };

        // Host Imports (aligned with wit/molt-runtime.wit)
        add_import("runtime_init", 0, &mut self.import_ids);
        add_import("runtime_shutdown", 0, &mut self.import_ids);
        add_import("sys_set_version_info", 29, &mut self.import_ids);
        add_import("print_obj", 1, &mut self.import_ids);
        add_import("print_newline", 8, &mut self.import_ids);
        add_import("alloc", 2, &mut self.import_ids);
        add_import("alloc_class", 3, &mut self.import_ids);
        add_import("alloc_class_trusted", 3, &mut self.import_ids);
        add_import("alloc_class_static", 3, &mut self.import_ids);
        add_import("async_sleep", 2, &mut self.import_ids);
        add_import("anext_default_poll", 2, &mut self.import_ids);
        add_import("asyncgen_poll", 2, &mut self.import_ids);
        add_import("promise_poll", 2, &mut self.import_ids);
        add_import("asyncio_wait_for_poll", 2, &mut self.import_ids);
        add_import("asyncio_wait_poll", 2, &mut self.import_ids);
        add_import("asyncio_gather_poll", 2, &mut self.import_ids);
        add_import("asyncio_socket_reader_read_poll", 2, &mut self.import_ids);
        add_import(
            "asyncio_socket_reader_readline_poll",
            2,
            &mut self.import_ids,
        );
        add_import("asyncio_stream_reader_read_poll", 2, &mut self.import_ids);
        add_import(
            "asyncio_stream_reader_readline_poll",
            2,
            &mut self.import_ids,
        );
        add_import("asyncio_stream_send_all_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_recv_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_connect_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_accept_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_recv_into_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_sendall_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_recvfrom_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_recvfrom_into_poll", 2, &mut self.import_ids);
        add_import("asyncio_sock_sendto_poll", 2, &mut self.import_ids);
        add_import("asyncio_timer_handle_poll", 2, &mut self.import_ids);
        add_import("asyncio_fd_watcher_poll", 2, &mut self.import_ids);
        add_import("asyncio_server_accept_loop_poll", 2, &mut self.import_ids);
        add_import("asyncio_ready_runner_poll", 2, &mut self.import_ids);
        add_import("contextlib_asyncgen_enter_poll", 2, &mut self.import_ids);
        add_import("contextlib_asyncgen_exit_poll", 2, &mut self.import_ids);
        add_import(
            "contextlib_async_exitstack_exit_poll",
            2,
            &mut self.import_ids,
        );
        add_import(
            "contextlib_async_exitstack_enter_context_poll",
            2,
            &mut self.import_ids,
        );
        add_import("asyncgen_new", 2, &mut self.import_ids);
        add_import("asyncgen_hooks_get", 0, &mut self.import_ids);
        add_import("asyncgen_hooks_set", 3, &mut self.import_ids);
        add_import("asyncgen_locals", 2, &mut self.import_ids);
        add_import("asyncgen_locals_register", 5, &mut self.import_ids);
        add_import("gen_locals", 2, &mut self.import_ids);
        add_import("gen_locals_register", 5, &mut self.import_ids);
        add_import("asyncgen_shutdown", 0, &mut self.import_ids);
        add_import("future_poll", 2, &mut self.import_ids);
        add_import("future_cancel", 2, &mut self.import_ids);
        add_import("future_cancel_msg", 3, &mut self.import_ids);
        add_import("future_cancel_clear", 2, &mut self.import_ids);
        add_import("promise_new", 0, &mut self.import_ids);
        add_import("promise_set_result", 3, &mut self.import_ids);
        add_import("promise_set_exception", 3, &mut self.import_ids);
        add_import("io_wait", 2, &mut self.import_ids);
        add_import("io_wait_new", 5, &mut self.import_ids);
        add_import("ws_wait", 2, &mut self.import_ids);
        add_import("ws_wait_new", 5, &mut self.import_ids);
        add_import("thread_submit", 5, &mut self.import_ids);
        add_import("thread_poll", 2, &mut self.import_ids);
        add_import("thread_spawn", 2, &mut self.import_ids);
        add_import("thread_join", 3, &mut self.import_ids);
        add_import("thread_is_alive", 2, &mut self.import_ids);
        add_import("thread_ident", 2, &mut self.import_ids);
        add_import("thread_native_id", 2, &mut self.import_ids);
        add_import("thread_current_ident", 0, &mut self.import_ids);
        add_import("thread_current_native_id", 0, &mut self.import_ids);
        add_import("thread_drop", 2, &mut self.import_ids);
        add_import("process_spawn", 9, &mut self.import_ids);
        add_import("process_wait_future", 2, &mut self.import_ids);
        add_import("process_poll", 2, &mut self.import_ids);
        add_import("process_pid", 2, &mut self.import_ids);
        add_import("process_returncode", 2, &mut self.import_ids);
        add_import("process_kill", 2, &mut self.import_ids);
        add_import("process_terminate", 2, &mut self.import_ids);
        add_import("process_stdin", 2, &mut self.import_ids);
        add_import("process_stdout", 2, &mut self.import_ids);
        add_import("process_stderr", 2, &mut self.import_ids);
        add_import("process_drop", 1, &mut self.import_ids);
        add_import("socket_constants", 0, &mut self.import_ids);
        add_import("socket_has_ipv6", 0, &mut self.import_ids);
        add_import("socket_new", 7, &mut self.import_ids);
        add_import("socket_close", 2, &mut self.import_ids);
        add_import("socket_drop", 1, &mut self.import_ids);
        add_import("socket_clone", 2, &mut self.import_ids);
        add_import("socket_fileno", 2, &mut self.import_ids);
        add_import("socket_gettimeout", 2, &mut self.import_ids);
        add_import("socket_settimeout", 3, &mut self.import_ids);
        add_import("socket_setblocking", 3, &mut self.import_ids);
        add_import("socket_getblocking", 2, &mut self.import_ids);
        add_import("socket_bind", 3, &mut self.import_ids);
        add_import("socket_listen", 3, &mut self.import_ids);
        add_import("socket_accept", 2, &mut self.import_ids);
        add_import("socket_connect", 3, &mut self.import_ids);
        add_import("socket_connect_ex", 3, &mut self.import_ids);
        add_import("socket_recv", 5, &mut self.import_ids);
        add_import("socket_recv_into", 7, &mut self.import_ids);
        add_import("socket_send", 5, &mut self.import_ids);
        add_import("socket_sendall", 5, &mut self.import_ids);
        add_import("socket_sendto", 7, &mut self.import_ids);
        add_import("socket_recvfrom", 5, &mut self.import_ids);
        add_import("socket_shutdown", 3, &mut self.import_ids);
        add_import("socket_getsockname", 2, &mut self.import_ids);
        add_import("socket_getpeername", 2, &mut self.import_ids);
        add_import("socket_setsockopt", 7, &mut self.import_ids);
        add_import("socket_getsockopt", 7, &mut self.import_ids);
        add_import("socket_detach", 2, &mut self.import_ids);
        add_import("socketpair", 5, &mut self.import_ids);
        add_import("socket_getaddrinfo", 9, &mut self.import_ids);
        add_import("socket_getnameinfo", 3, &mut self.import_ids);
        add_import("socket_gethostname", 0, &mut self.import_ids);
        add_import("socket_getservbyname", 3, &mut self.import_ids);
        add_import("socket_getservbyport", 3, &mut self.import_ids);
        add_import("socket_inet_pton", 3, &mut self.import_ids);
        add_import("socket_inet_ntop", 3, &mut self.import_ids);
        add_import("os_close", 2, &mut self.import_ids);
        add_import("os_dup", 2, &mut self.import_ids);
        add_import("os_get_inheritable", 2, &mut self.import_ids);
        add_import("os_set_inheritable", 3, &mut self.import_ids);
        add_import("os_urandom", 2, &mut self.import_ids);
        add_import("sleep_register", 27, &mut self.import_ids);
        add_import("block_on", 2, &mut self.import_ids);
        add_import("spawn", 1, &mut self.import_ids);
        add_import("cancel_token_new", 2, &mut self.import_ids);
        add_import("cancel_token_clone", 2, &mut self.import_ids);
        add_import("cancel_token_drop", 2, &mut self.import_ids);
        add_import("cancel_token_cancel", 2, &mut self.import_ids);
        add_import("cancel_token_is_cancelled", 2, &mut self.import_ids);
        add_import("cancel_token_set_current", 2, &mut self.import_ids);
        add_import("cancel_token_get_current", 0, &mut self.import_ids);
        add_import("cancelled", 0, &mut self.import_ids);
        add_import("cancel_current", 0, &mut self.import_ids);
        add_import("lock_new", 0, &mut self.import_ids);
        add_import("lock_acquire", 5, &mut self.import_ids);
        add_import("lock_release", 2, &mut self.import_ids);
        add_import("lock_locked", 2, &mut self.import_ids);
        add_import("lock_drop", 2, &mut self.import_ids);
        add_import("rlock_new", 0, &mut self.import_ids);
        add_import("rlock_acquire", 5, &mut self.import_ids);
        add_import("rlock_release", 2, &mut self.import_ids);
        add_import("rlock_locked", 2, &mut self.import_ids);
        add_import("rlock_drop", 2, &mut self.import_ids);
        add_import("chan_new", 2, &mut self.import_ids);
        add_import("chan_send", 3, &mut self.import_ids);
        add_import("chan_send_blocking", 3, &mut self.import_ids);
        add_import("chan_try_send", 3, &mut self.import_ids);
        add_import("chan_recv", 2, &mut self.import_ids);
        add_import("chan_recv_blocking", 2, &mut self.import_ids);
        add_import("chan_try_recv", 2, &mut self.import_ids);
        add_import("chan_drop", 2, &mut self.import_ids);
        add_import("add", 3, &mut self.import_ids);
        add_import("inplace_add", 3, &mut self.import_ids);
        add_import("vec_sum_int", 3, &mut self.import_ids);
        add_import("vec_sum_int_trusted", 3, &mut self.import_ids);
        add_import("vec_sum_int_range_iter", 3, &mut self.import_ids);
        add_import("vec_sum_int_range_iter_trusted", 3, &mut self.import_ids);
        add_import("vec_sum_int_range", 5, &mut self.import_ids);
        add_import("vec_sum_int_range_trusted", 5, &mut self.import_ids);
        add_import("vec_sum_float", 3, &mut self.import_ids);
        add_import("vec_sum_float_trusted", 3, &mut self.import_ids);
        add_import("vec_sum_float_range_iter", 3, &mut self.import_ids);
        add_import("vec_sum_float_range_iter_trusted", 3, &mut self.import_ids);
        add_import("vec_sum_float_range", 5, &mut self.import_ids);
        add_import("vec_sum_float_range_trusted", 5, &mut self.import_ids);
        add_import("vec_prod_int", 3, &mut self.import_ids);
        add_import("vec_prod_int_trusted", 3, &mut self.import_ids);
        add_import("vec_prod_int_range", 5, &mut self.import_ids);
        add_import("vec_prod_int_range_trusted", 5, &mut self.import_ids);
        add_import("vec_min_int", 3, &mut self.import_ids);
        add_import("vec_min_int_trusted", 3, &mut self.import_ids);
        add_import("vec_min_int_range", 5, &mut self.import_ids);
        add_import("vec_min_int_range_trusted", 5, &mut self.import_ids);
        add_import("vec_max_int", 3, &mut self.import_ids);
        add_import("vec_max_int_trusted", 3, &mut self.import_ids);
        add_import("vec_max_int_range", 5, &mut self.import_ids);
        add_import("vec_max_int_range_trusted", 5, &mut self.import_ids);
        add_import("sub", 3, &mut self.import_ids);
        add_import("mul", 3, &mut self.import_ids);
        add_import("inplace_sub", 3, &mut self.import_ids);
        add_import("inplace_mul", 3, &mut self.import_ids);
        add_import("bit_or", 3, &mut self.import_ids);
        add_import("bit_and", 3, &mut self.import_ids);
        add_import("bit_xor", 3, &mut self.import_ids);
        add_import("invert", 2, &mut self.import_ids);
        add_import("inplace_bit_or", 3, &mut self.import_ids);
        add_import("inplace_bit_and", 3, &mut self.import_ids);
        add_import("inplace_bit_xor", 3, &mut self.import_ids);
        add_import("lshift", 3, &mut self.import_ids);
        add_import("rshift", 3, &mut self.import_ids);
        add_import("matmul", 3, &mut self.import_ids);
        add_import("div", 3, &mut self.import_ids);
        add_import("floordiv", 3, &mut self.import_ids);
        add_import("mod", 3, &mut self.import_ids);
        add_import("pow", 3, &mut self.import_ids);
        add_import("pow_mod", 5, &mut self.import_ids);
        add_import("round", 5, &mut self.import_ids);
        add_import("trunc", 2, &mut self.import_ids);
        add_import("lt", 3, &mut self.import_ids);
        add_import("le", 3, &mut self.import_ids);
        add_import("gt", 3, &mut self.import_ids);
        add_import("ge", 3, &mut self.import_ids);
        add_import("eq", 3, &mut self.import_ids);
        add_import("ne", 3, &mut self.import_ids);
        add_import("string_eq", 3, &mut self.import_ids);
        add_import("is", 3, &mut self.import_ids);
        add_import("closure_load", 16, &mut self.import_ids);
        add_import("closure_store", 17, &mut self.import_ids);
        add_import("not", 2, &mut self.import_ids);
        add_import("contains", 3, &mut self.import_ids);
        add_import("guard_type", 3, &mut self.import_ids);
        add_import("guard_layout_ptr", 17, &mut self.import_ids);
        add_import("guarded_field_get_ptr", 21, &mut self.import_ids);
        add_import("guarded_field_set_ptr", 22, &mut self.import_ids);
        add_import("guarded_field_init_ptr", 22, &mut self.import_ids);
        add_import("handle_resolve", 13, &mut self.import_ids);
        add_import("inc_ref_obj", 1, &mut self.import_ids);
        add_import("dec_ref_obj", 1, &mut self.import_ids);
        add_import("get_attr_generic", 23, &mut self.import_ids);
        add_import("get_attr_ptr", 23, &mut self.import_ids);
        add_import("get_attr_object", 18, &mut self.import_ids);
        add_import("get_attr_object_ic", 25, &mut self.import_ids);
        add_import("get_attr_special", 18, &mut self.import_ids);
        add_import("set_attr_generic", 24, &mut self.import_ids);
        add_import("set_attr_ptr", 24, &mut self.import_ids);
        add_import("set_attr_object", 25, &mut self.import_ids);
        add_import("del_attr_generic", 23, &mut self.import_ids);
        add_import("del_attr_ptr", 23, &mut self.import_ids);
        add_import("del_attr_object", 18, &mut self.import_ids);
        add_import("object_field_get", 3, &mut self.import_ids);
        add_import("object_field_get_ptr", 16, &mut self.import_ids);
        add_import("object_field_set", 5, &mut self.import_ids);
        add_import("object_field_set_ptr", 17, &mut self.import_ids);
        add_import("object_field_init", 5, &mut self.import_ids);
        add_import("object_field_init_ptr", 17, &mut self.import_ids);
        add_import("module_new", 2, &mut self.import_ids);
        add_import("module_cache_get", 2, &mut self.import_ids);
        add_import("module_import", 2, &mut self.import_ids);
        add_import("module_cache_set", 3, &mut self.import_ids);
        add_import("module_get_attr", 3, &mut self.import_ids);
        add_import("module_get_global", 3, &mut self.import_ids);
        add_import("module_del_global", 3, &mut self.import_ids);
        add_import("module_get_name", 3, &mut self.import_ids);
        add_import("module_set_attr", 5, &mut self.import_ids);
        add_import("module_import_star", 3, &mut self.import_ids);
        add_import("get_attr_name", 3, &mut self.import_ids);
        add_import("get_attr_name_default", 5, &mut self.import_ids);
        add_import("has_attr_name", 3, &mut self.import_ids);
        add_import("set_attr_name", 5, &mut self.import_ids);
        add_import("del_attr_name", 3, &mut self.import_ids);
        add_import("is_truthy", 2, &mut self.import_ids);
        add_import("is_bound_method", 2, &mut self.import_ids);
        add_import("is_function_obj", 2, &mut self.import_ids);
        add_import("function_default_kind", 2, &mut self.import_ids);
        add_import("function_closure_bits", 2, &mut self.import_ids);
        add_import("function_is_generator", 2, &mut self.import_ids);
        add_import("function_is_coroutine", 2, &mut self.import_ids);
        add_import("function_set_builtin", 2, &mut self.import_ids);
        add_import("call_arity_error", 3, &mut self.import_ids);
        add_import("missing", 0, &mut self.import_ids);
        add_import("pending", 0, &mut self.import_ids);
        add_import("not_implemented", 0, &mut self.import_ids);
        add_import("ellipsis", 0, &mut self.import_ids);
        add_import("json_parse_scalar", 19, &mut self.import_ids);
        add_import("msgpack_parse_scalar", 19, &mut self.import_ids);
        add_import("cbor_parse_scalar", 19, &mut self.import_ids);
        add_import("json_parse_scalar_obj", 2, &mut self.import_ids);
        add_import("msgpack_parse_scalar_obj", 2, &mut self.import_ids);
        add_import("cbor_parse_scalar_obj", 2, &mut self.import_ids);
        add_import("struct_calcsize", 2, &mut self.import_ids);
        add_import("struct_pack", 3, &mut self.import_ids);
        add_import("struct_unpack", 3, &mut self.import_ids);
        add_import("struct_pack_into", 5, &mut self.import_ids);
        add_import("struct_unpack_from", 5, &mut self.import_ids);
        add_import("struct_iter_unpack", 3, &mut self.import_ids);
        add_import("weakref_register", 5, &mut self.import_ids);
        add_import("weakref_get", 2, &mut self.import_ids);
        add_import("weakref_drop", 2, &mut self.import_ids);
        add_import("string_from_bytes", 19, &mut self.import_ids);
        add_import("bytes_from_bytes", 19, &mut self.import_ids);
        add_import("bigint_from_str", 16, &mut self.import_ids);
        add_import("str_from_obj", 2, &mut self.import_ids);
        add_import("repr_from_obj", 2, &mut self.import_ids);
        add_import("repr_builtin", 2, &mut self.import_ids);
        add_import("format_builtin", 3, &mut self.import_ids);
        add_import("ascii_from_obj", 2, &mut self.import_ids);
        add_import("bin_builtin", 2, &mut self.import_ids);
        add_import("oct_builtin", 2, &mut self.import_ids);
        add_import("hex_builtin", 2, &mut self.import_ids);
        add_import("callable_builtin", 2, &mut self.import_ids);
        add_import("int_from_obj", 5, &mut self.import_ids);
        add_import("float_from_obj", 2, &mut self.import_ids);
        add_import("complex_from_obj", 5, &mut self.import_ids);
        add_import("memoryview_new", 2, &mut self.import_ids);
        add_import("memoryview_tobytes", 2, &mut self.import_ids);
        add_import("memoryview_cast", 7, &mut self.import_ids);
        add_import("intarray_from_seq", 2, &mut self.import_ids);
        add_import("len", 2, &mut self.import_ids);
        add_import("id", 2, &mut self.import_ids);
        add_import("hash_builtin", 2, &mut self.import_ids);
        add_import("ord", 2, &mut self.import_ids);
        add_import("chr", 2, &mut self.import_ids);
        add_import("abs_builtin", 2, &mut self.import_ids);
        // NOTE(multi-value §3.1): divmod always returns exactly 2 values.
        // When the host-side import is updated to use the multi-value ABI,
        // change the type index from 3 (i64,i64)->i64 to
        // MULTI_RETURN_2_TYPE (i64,i64)->(i64,i64) and update call sites
        // to consume the two stack results directly instead of calling
        // tuple_index on the returned handle.
        add_import("divmod_builtin", 3, &mut self.import_ids);
        add_import("open_builtin", 28, &mut self.import_ids);
        add_import("getargv", 0, &mut self.import_ids);
        add_import("sys_version_info", 0, &mut self.import_ids);
        add_import("sys_version", 0, &mut self.import_ids);
        add_import("sys_hexversion", 0, &mut self.import_ids);
        add_import("sys_api_version", 0, &mut self.import_ids);
        add_import("sys_abiflags", 0, &mut self.import_ids);
        add_import("sys_implementation_payload", 0, &mut self.import_ids);
        add_import("sys_stdin", 0, &mut self.import_ids);
        add_import("sys_stdout", 0, &mut self.import_ids);
        add_import("sys_stderr", 0, &mut self.import_ids);
        add_import("sys_executable", 0, &mut self.import_ids);
        add_import("getrecursionlimit", 0, &mut self.import_ids);
        add_import("setrecursionlimit", 2, &mut self.import_ids);
        add_import("recursion_guard_enter", 0, &mut self.import_ids);
        add_import("recursion_guard_exit", 8, &mut self.import_ids);
        add_import("trace_enter_slot", 2, &mut self.import_ids);
        // Compiler-emitted: pin the current frame's locals dict so builtins.locals() can return
        // a stable, alias-call-safe mapping without CPython-style fast-locals introspection.
        add_import("frame_locals_set", 2, &mut self.import_ids);
        add_import("trace_set_line", 2, &mut self.import_ids);
        add_import("trace_exit", 0, &mut self.import_ids);
        add_import("code_slots_init", 2, &mut self.import_ids);
        add_import("code_slot_set", 3, &mut self.import_ids);
        add_import("fn_ptr_code_set", 3, &mut self.import_ids);
        add_import("code_new", 28, &mut self.import_ids);
        add_import("compile_builtin", 9, &mut self.import_ids);
        add_import("round_builtin", 3, &mut self.import_ids);
        add_import("enumerate_builtin", 3, &mut self.import_ids);
        add_import("iter_sentinel", 3, &mut self.import_ids);
        add_import("next_builtin", 3, &mut self.import_ids);
        add_import("any_builtin", 2, &mut self.import_ids);
        add_import("all_builtin", 2, &mut self.import_ids);
        add_import("sum_builtin", 3, &mut self.import_ids);
        add_import("min_builtin", 5, &mut self.import_ids);
        add_import("max_builtin", 5, &mut self.import_ids);
        add_import("sorted_builtin", 5, &mut self.import_ids);
        add_import("map_builtin", 3, &mut self.import_ids);
        add_import("filter_builtin", 3, &mut self.import_ids);
        add_import("zip_builtin", 3, &mut self.import_ids);
        add_import("reversed_builtin", 2, &mut self.import_ids);
        add_import("getattr_builtin", 5, &mut self.import_ids);
        add_import("dir_builtin", 2, &mut self.import_ids);
        add_import("vars_builtin", 2, &mut self.import_ids);
        add_import("anext_builtin", 3, &mut self.import_ids);
        add_import("func_new_builtin", 5, &mut self.import_ids);
        add_import("print_builtin", 12, &mut self.import_ids);
        add_import("super_builtin", 3, &mut self.import_ids);
        add_import("callargs_new", 3, &mut self.import_ids);
        add_import("callargs_push_pos", 3, &mut self.import_ids);
        add_import("callargs_push_kw", 5, &mut self.import_ids);
        add_import("callargs_expand_star", 3, &mut self.import_ids);
        add_import("callargs_expand_kwstar", 3, &mut self.import_ids);
        add_import("call_bind", 3, &mut self.import_ids);
        add_import("call_bind_ic", 5, &mut self.import_ids);
        add_import("call_indirect_ic", 5, &mut self.import_ids);
        add_import("invoke_ffi_ic", 7, &mut self.import_ids);
        add_import("slice", 5, &mut self.import_ids);
        add_import("slice_new", 5, &mut self.import_ids);
        add_import("range_new", 5, &mut self.import_ids);
        add_import("list_from_range", 5, &mut self.import_ids);
        add_import("list_builder_new", 2, &mut self.import_ids);
        add_import("list_builder_append", 6, &mut self.import_ids);
        add_import("list_builder_finish", 2, &mut self.import_ids);
        add_import("tuple_builder_finish", 2, &mut self.import_ids);
        add_import("list_append", 3, &mut self.import_ids);
        add_import("list_pop", 3, &mut self.import_ids);
        add_import("list_extend", 3, &mut self.import_ids);
        add_import("list_insert", 5, &mut self.import_ids);
        add_import("list_remove", 3, &mut self.import_ids);
        add_import("list_clear", 2, &mut self.import_ids);
        add_import("list_copy", 2, &mut self.import_ids);
        add_import("list_reverse", 2, &mut self.import_ids);
        add_import("list_sort", 5, &mut self.import_ids);
        add_import("list_count", 3, &mut self.import_ids);
        add_import("list_index", 3, &mut self.import_ids);
        add_import("list_index_range", 7, &mut self.import_ids);
        add_import("heapq_heapify", 2, &mut self.import_ids);
        add_import("heapq_heappush", 3, &mut self.import_ids);
        add_import("heapq_heappop", 2, &mut self.import_ids);
        add_import("heapq_heapreplace", 3, &mut self.import_ids);
        add_import("heapq_heappushpop", 3, &mut self.import_ids);
        add_import("tuple_from_list", 2, &mut self.import_ids);
        add_import("dict_new", 2, &mut self.import_ids);
        add_import("dict_from_obj", 2, &mut self.import_ids);
        add_import("dict_set", 5, &mut self.import_ids);
        add_import("dict_get", 5, &mut self.import_ids);
        add_import("dict_inc", 5, &mut self.import_ids);
        add_import("dict_str_int_inc", 5, &mut self.import_ids);
        add_import("string_split_ws_dict_inc", 5, &mut self.import_ids);
        add_import("string_split_sep_dict_inc", 7, &mut self.import_ids);
        add_import("taq_ingest_line", 5, &mut self.import_ids);
        add_import("dict_pop", 7, &mut self.import_ids);
        add_import("dict_setdefault", 5, &mut self.import_ids);
        add_import("dict_setdefault_empty_list", 3, &mut self.import_ids);
        add_import("dict_update", 3, &mut self.import_ids);
        add_import("dict_clear", 2, &mut self.import_ids);
        add_import("dict_copy", 2, &mut self.import_ids);
        add_import("dict_popitem", 2, &mut self.import_ids);
        add_import("dict_update_kwstar", 3, &mut self.import_ids);
        add_import("dict_keys", 2, &mut self.import_ids);
        add_import("dict_values", 2, &mut self.import_ids);
        add_import("dict_items", 2, &mut self.import_ids);
        add_import("set_new", 2, &mut self.import_ids);
        add_import("set_add", 3, &mut self.import_ids);
        add_import("set_discard", 3, &mut self.import_ids);
        add_import("set_remove", 3, &mut self.import_ids);
        add_import("set_pop", 2, &mut self.import_ids);
        add_import("set_update", 3, &mut self.import_ids);
        add_import("set_intersection_update", 3, &mut self.import_ids);
        add_import("set_difference_update", 3, &mut self.import_ids);
        add_import("set_symdiff_update", 3, &mut self.import_ids);
        add_import("frozenset_new", 2, &mut self.import_ids);
        add_import("frozenset_add", 3, &mut self.import_ids);
        add_import("tuple_count", 3, &mut self.import_ids);
        add_import("tuple_index", 3, &mut self.import_ids);
        add_import("iter", 2, &mut self.import_ids);
        add_import("enumerate", 5, &mut self.import_ids);
        add_import("aiter", 2, &mut self.import_ids);
        add_import("iter_next", 2, &mut self.import_ids);
        add_import("anext", 2, &mut self.import_ids);
        add_import("task_new", 5, &mut self.import_ids);
        add_import("task_register_token_owned", 3, &mut self.import_ids);
        add_import("generator_send", 3, &mut self.import_ids);
        add_import("generator_throw", 3, &mut self.import_ids);
        add_import("generator_close", 2, &mut self.import_ids);
        add_import("is_generator", 2, &mut self.import_ids);
        add_import("is_callable", 2, &mut self.import_ids);
        add_import("index", 3, &mut self.import_ids);
        add_import("store_index", 5, &mut self.import_ids);
        add_import("del_index", 3, &mut self.import_ids);
        add_import("bytes_find", 3, &mut self.import_ids);
        add_import("bytearray_find", 3, &mut self.import_ids);
        add_import("string_find", 3, &mut self.import_ids);
        add_import("bytes_find_slice", 9, &mut self.import_ids);
        add_import("bytearray_find_slice", 9, &mut self.import_ids);
        add_import("string_find_slice", 9, &mut self.import_ids);
        add_import("string_format", 3, &mut self.import_ids);
        add_import("string_startswith", 3, &mut self.import_ids);
        add_import("bytes_startswith", 3, &mut self.import_ids);
        add_import("bytearray_startswith", 3, &mut self.import_ids);
        add_import("string_startswith_slice", 9, &mut self.import_ids);
        add_import("bytes_startswith_slice", 9, &mut self.import_ids);
        add_import("bytearray_startswith_slice", 9, &mut self.import_ids);
        add_import("string_endswith", 3, &mut self.import_ids);
        add_import("bytes_endswith", 3, &mut self.import_ids);
        add_import("bytearray_endswith", 3, &mut self.import_ids);
        add_import("string_endswith_slice", 9, &mut self.import_ids);
        add_import("bytes_endswith_slice", 9, &mut self.import_ids);
        add_import("bytearray_endswith_slice", 9, &mut self.import_ids);
        add_import("string_count", 3, &mut self.import_ids);
        add_import("bytes_count", 3, &mut self.import_ids);
        add_import("bytearray_count", 3, &mut self.import_ids);
        add_import("string_count_slice", 9, &mut self.import_ids);
        add_import("bytes_count_slice", 9, &mut self.import_ids);
        add_import("bytearray_count_slice", 9, &mut self.import_ids);
        add_import("env_get", 3, &mut self.import_ids);
        add_import("env_snapshot", 0, &mut self.import_ids);
        add_import("os_name", 0, &mut self.import_ids);
        for (import_name, arity) in [
            ("importlib_bootstrap_payload", 2usize),
            ("importlib_cache_from_source", 1),
            ("importlib_coerce_module_name", 3),
            ("importlib_decode_source", 1),
            ("importlib_ensure_default_meta_path", 1),
            ("importlib_exec_extension", 3),
            ("importlib_exec_restricted_source", 3),
            ("importlib_exec_sourceless", 3),
            ("importlib_extension_loader_payload", 3),
            ("importlib_filefinder_find_spec", 3),
            ("importlib_filefinder_invalidate", 1),
            ("importlib_find_in_path", 2),
            ("importlib_find_in_path_package_context", 2),
            ("importlib_find_spec", 8),
            ("importlib_find_spec_orchestrate", 5),
            ("importlib_frozen_external_payload", 2),
            ("importlib_frozen_payload", 2),
            ("importlib_import_module", 3),
            ("importlib_import_optional", 1),
            ("importlib_import_or_fallback", 2),
            ("importlib_import_required", 1),
            ("importlib_invalidate_caches", 0),
            ("importlib_known_absent_missing_name", 1),
            ("importlib_load_module_shim", 3),
            ("importlib_metadata_dist_paths", 2),
            ("importlib_metadata_distributions_payload", 2),
            ("importlib_metadata_entry_points_filter_payload", 5),
            ("importlib_metadata_entry_points_select_payload", 4),
            ("importlib_metadata_normalize_name", 1),
            ("importlib_metadata_packages_distributions_payload", 2),
            ("importlib_metadata_payload", 1),
            ("importlib_metadata_record_payload", 1),
            ("importlib_metadata_types_payload", 4),
            ("importlib_module_from_spec", 1),
            ("importlib_module_spec_is_package", 1),
            ("importlib_package_root_from_origin", 1),
            ("importlib_path_is_archive_member", 1),
            ("importlib_pathfinder_find_spec", 3),
            ("importlib_read_file", 1),
            ("importlib_reload", 4),
            ("importlib_resolve_name", 2),
            ("importlib_resources_as_file_enter", 2),
            ("importlib_resources_as_file_exit", 3),
            ("importlib_resources_contents_from_package", 3),
            ("importlib_resources_contents_from_package_parts", 4),
            ("importlib_resources_files_payload", 4),
            ("importlib_resources_is_resource_from_package", 4),
            ("importlib_resources_is_resource_from_package_parts", 4),
            ("importlib_resources_joinpath", 2),
            ("importlib_resources_loader_reader", 2),
            ("importlib_resources_module_name", 2),
            ("importlib_resources_normalize_path", 1),
            ("importlib_resources_only", 3),
            ("importlib_resources_open_mode_is_text", 1),
            ("importlib_resources_open_resource_bytes_from_package", 4),
            (
                "importlib_resources_open_resource_bytes_from_package_parts",
                4,
            ),
            ("importlib_resources_package_info", 3),
            ("importlib_resources_package_leaf_name", 1),
            ("importlib_resources_path_payload", 1),
            ("importlib_resources_read_text_from_package", 6),
            ("importlib_resources_read_text_from_package_parts", 6),
            ("importlib_resources_reader_child_names", 2),
            ("importlib_resources_reader_contents", 1),
            ("importlib_resources_reader_contents_from_roots", 1),
            ("importlib_resources_reader_exists", 2),
            ("importlib_resources_reader_files_traversable", 1),
            ("importlib_resources_reader_is_dir", 2),
            ("importlib_resources_reader_is_resource", 2),
            ("importlib_resources_reader_is_resource_from_roots", 2),
            ("importlib_resources_reader_open_resource_bytes", 2),
            (
                "importlib_resources_reader_open_resource_bytes_from_roots",
                2,
            ),
            ("importlib_resources_reader_resource_path", 2),
            ("importlib_resources_reader_resource_path_from_roots", 2),
            ("importlib_resources_reader_roots", 1),
            ("importlib_resources_resource_path_from_package", 4),
            ("importlib_resources_resource_path_from_package_parts", 4),
            ("importlib_runtime_modules", 0),
            ("importlib_set_module_state", 8),
            ("importlib_source_exec_payload", 3),
            ("importlib_source_from_cache", 1),
            ("importlib_source_hash", 1),
            ("importlib_sourceless_loader_payload", 3),
            ("importlib_spec_from_file_location", 5),
            ("importlib_spec_from_loader", 5),
            ("importlib_stabilize_module_state", 6),
            ("importlib_validate_resource_name", 1),
            ("importlib_zip_read_entry", 2),
            ("importlib_zip_source_exec_payload", 4),
            ("os_access", 2usize),
            ("os_altsep", 0),
            ("os_chdir", 1),
            ("os_chmod", 2),
            ("os_cpu_count", 0),
            ("os_curdir", 0),
            ("os_devnull", 0),
            ("os_dup2", 2),
            ("os_extsep", 0),
            ("os_fdopen", 3),
            ("os_fsencode", 1),
            ("os_fspath", 1),
            ("os_fstat", 1),
            ("os_ftruncate", 2),
            ("os_get_terminal_size", 1),
            ("os_getcwd", 0),
            ("os_getegid", 0),
            ("os_geteuid", 0),
            ("os_getgid", 0),
            ("os_getloadavg", 0),
            ("os_getlogin", 0),
            ("os_getpgrp", 0),
            ("os_getpid", 0),
            ("os_getppid", 0),
            ("os_getuid", 0),
            ("os_isatty", 1),
            ("os_kill", 2),
            ("os_linesep", 0),
            ("os_link", 2),
            ("os_listdir", 1),
            ("os_lseek", 3),
            ("os_lstat", 1),
            ("os_mkdir", 2),
            ("os_open", 3),
            ("os_open_flags", 0),
            ("os_pardir", 0),
            ("os_path_commonpath", 1),
            ("os_path_commonprefix", 1),
            ("os_path_getatime", 1),
            ("os_path_getctime", 1),
            ("os_path_getmtime", 1),
            ("os_path_getsize", 1),
            ("os_path_realpath", 1),
            ("os_path_samefile", 2),
            ("os_pathsep", 0),
            ("os_pipe", 0),
            ("os_read", 2),
            ("os_readlink", 1),
            ("os_removedirs", 1),
            ("os_rename", 2),
            ("os_replace", 2),
            ("os_rmdir", 1),
            ("os_scandir", 1),
            ("os_sendfile", 4),
            ("os_sep", 0),
            ("os_setpgrp", 0),
            ("os_setsid", 0),
            ("os_stat", 1),
            ("os_symlink", 2),
            ("os_sysconf", 1),
            ("os_sysconf_names", 0),
            ("os_truncate", 2),
            ("os_umask", 1),
            ("os_uname", 0),
            ("os_utime", 3),
            ("os_waitpid", 2),
            ("os_walk", 3),
            ("os_wexitstatus", 1),
            ("os_wifexited", 1),
            ("os_wifsignaled", 1),
            ("os_wifstopped", 1),
            ("os_write", 2),
            ("os_wstopsig", 1),
            ("os_wtermsig", 1),
        ] {
            add_import(
                import_name,
                simple_i64_import_type(arity),
                &mut self.import_ids,
            );
        }
        add_import("sys_platform", 0, &mut self.import_ids);
        add_import("errno_constants", 0, &mut self.import_ids);
        add_import("getpid", 0, &mut self.import_ids);
        add_import("getframe", 2, &mut self.import_ids);
        add_import("getcwd", 0, &mut self.import_ids);
        add_import("time_monotonic", 0, &mut self.import_ids);
        add_import("time_monotonic_ns", 0, &mut self.import_ids);
        add_import("time_perf_counter", 0, &mut self.import_ids);
        add_import("time_perf_counter_ns", 0, &mut self.import_ids);
        add_import("time_process_time", 0, &mut self.import_ids);
        add_import("time_process_time_ns", 0, &mut self.import_ids);
        add_import("time_time", 0, &mut self.import_ids);
        add_import("time_time_ns", 0, &mut self.import_ids);
        add_import("time_localtime", 2, &mut self.import_ids);
        add_import("time_gmtime", 2, &mut self.import_ids);
        add_import("time_strftime", 3, &mut self.import_ids);
        add_import("time_timezone", 0, &mut self.import_ids);
        add_import("time_tzname", 0, &mut self.import_ids);
        add_import("math_log", 2, &mut self.import_ids);
        add_import("math_log2", 2, &mut self.import_ids);
        add_import("math_exp", 2, &mut self.import_ids);
        add_import("math_sin", 2, &mut self.import_ids);
        add_import("math_cos", 2, &mut self.import_ids);
        add_import("math_acos", 2, &mut self.import_ids);
        add_import("math_lgamma", 2, &mut self.import_ids);
        add_import("path_exists", 2, &mut self.import_ids);
        add_import("path_listdir", 2, &mut self.import_ids);
        add_import("path_mkdir", 3, &mut self.import_ids);
        add_import("path_unlink", 2, &mut self.import_ids);
        add_import("path_rmdir", 2, &mut self.import_ids);
        add_import("path_chmod", 3, &mut self.import_ids);
        add_import("string_join", 3, &mut self.import_ids);
        add_import("string_split", 3, &mut self.import_ids);
        add_import("string_split_max", 5, &mut self.import_ids);
        add_import("statistics_mean_slice", 12, &mut self.import_ids);
        add_import("statistics_stdev_slice", 12, &mut self.import_ids);
        add_import("string_lower", 2, &mut self.import_ids);
        add_import("string_upper", 2, &mut self.import_ids);
        add_import("string_capitalize", 2, &mut self.import_ids);
        add_import("string_strip", 3, &mut self.import_ids);
        add_import("string_lstrip", 3, &mut self.import_ids);
        add_import("string_rstrip", 3, &mut self.import_ids);
        add_import("bytes_split", 3, &mut self.import_ids);
        add_import("bytes_split_max", 5, &mut self.import_ids);
        add_import("bytearray_split", 3, &mut self.import_ids);
        add_import("bytearray_split_max", 5, &mut self.import_ids);
        add_import("string_replace", 7, &mut self.import_ids);
        add_import("bytes_replace", 7, &mut self.import_ids);
        add_import("bytearray_replace", 7, &mut self.import_ids);
        add_import("bytes_from_obj", 2, &mut self.import_ids);
        add_import("bytearray_from_obj", 2, &mut self.import_ids);
        add_import("bytes_from_str", 5, &mut self.import_ids);
        add_import("bytearray_from_str", 5, &mut self.import_ids);
        add_import("buffer2d_new", 5, &mut self.import_ids);
        add_import("buffer2d_get", 5, &mut self.import_ids);
        add_import("buffer2d_set", 7, &mut self.import_ids);
        add_import("buffer2d_matmul", 3, &mut self.import_ids);
        add_import("dataclass_new", 7, &mut self.import_ids);
        add_import("dataclass_get", 3, &mut self.import_ids);
        add_import("dataclass_set", 5, &mut self.import_ids);
        add_import("dataclass_set_class", 3, &mut self.import_ids);
        add_import("class_new", 2, &mut self.import_ids);
        add_import("class_set_base", 3, &mut self.import_ids);
        add_import("class_apply_set_name", 2, &mut self.import_ids);
        add_import("super_new", 3, &mut self.import_ids);
        add_import("builtin_type", 2, &mut self.import_ids);
        add_import("type_of", 2, &mut self.import_ids);
        add_import("class_layout_version", 2, &mut self.import_ids);
        add_import("class_set_layout_version", 3, &mut self.import_ids);
        add_import("isinstance", 3, &mut self.import_ids);
        add_import("issubclass", 3, &mut self.import_ids);
        add_import("object_new", 0, &mut self.import_ids);
        add_import("func_new", 5, &mut self.import_ids);
        add_import("func_new_closure", 7, &mut self.import_ids);
        add_import("bound_method_new", 3, &mut self.import_ids);
        add_import("classmethod_new", 2, &mut self.import_ids);
        add_import("staticmethod_new", 2, &mut self.import_ids);
        add_import("property_new", 5, &mut self.import_ids);
        add_import("object_set_class", 16, &mut self.import_ids);
        add_import("stream_new", 2, &mut self.import_ids);
        add_import("stream_clone", 2, &mut self.import_ids);
        add_import("stream_send", 18, &mut self.import_ids);
        add_import("stream_send_obj", 3, &mut self.import_ids);
        add_import("stream_recv", 2, &mut self.import_ids);
        add_import("stream_close", 1, &mut self.import_ids);
        add_import("stream_drop", 1, &mut self.import_ids);
        add_import("ws_connect", 19, &mut self.import_ids);
        add_import("ws_pair", 20, &mut self.import_ids);
        add_import("ws_send", 18, &mut self.import_ids);
        add_import("ws_connect_obj", 2, &mut self.import_ids);
        add_import("ws_pair_obj", 2, &mut self.import_ids);
        add_import("ws_send_obj", 3, &mut self.import_ids);
        add_import("ws_recv", 2, &mut self.import_ids);
        add_import("ws_close", 1, &mut self.import_ids);
        add_import("ws_drop", 1, &mut self.import_ids);
        add_import("context_null", 2, &mut self.import_ids);
        add_import("context_enter", 2, &mut self.import_ids);
        add_import("context_exit", 3, &mut self.import_ids);
        add_import("context_unwind", 2, &mut self.import_ids);
        add_import("context_depth", 0, &mut self.import_ids);
        add_import("context_unwind_to", 3, &mut self.import_ids);
        add_import("context_closing", 2, &mut self.import_ids);
        add_import("exception_push", 0, &mut self.import_ids);
        add_import("exception_pop", 0, &mut self.import_ids);
        add_import("exception_stack_clear", 0, &mut self.import_ids);
        add_import("exception_last", 0, &mut self.import_ids);
        add_import("exception_active", 0, &mut self.import_ids);
        add_import("exception_new", 3, &mut self.import_ids);
        add_import("exception_new_from_class", 3, &mut self.import_ids);
        add_import("exceptiongroup_match", 3, &mut self.import_ids);
        add_import("exceptiongroup_combine", 2, &mut self.import_ids);
        add_import("exception_clear", 0, &mut self.import_ids);
        add_import("exception_pending", 0, &mut self.import_ids);
        add_import("exception_kind", 2, &mut self.import_ids);
        add_import("exception_class", 2, &mut self.import_ids);
        add_import("exception_message", 2, &mut self.import_ids);
        add_import("exception_set_cause", 3, &mut self.import_ids);
        add_import("exception_set_value", 3, &mut self.import_ids);
        add_import("exception_context_set", 2, &mut self.import_ids);
        add_import("exception_set_last", 2, &mut self.import_ids);
        add_import("raise", 2, &mut self.import_ids);
        add_import("bridge_unavailable", 2, &mut self.import_ids);
        add_import("db_query", 26, &mut self.import_ids);
        add_import("db_exec", 26, &mut self.import_ids);
        add_import("db_query_obj", 3, &mut self.import_ids);
        add_import("db_exec_obj", 3, &mut self.import_ids);
        add_import("file_open", 3, &mut self.import_ids);
        add_import("file_read", 3, &mut self.import_ids);
        add_import("file_readline", 3, &mut self.import_ids);
        add_import("file_readlines", 3, &mut self.import_ids);
        add_import("file_readinto", 3, &mut self.import_ids);
        add_import("file_readinto1", 3, &mut self.import_ids);
        add_import("file_write", 3, &mut self.import_ids);
        add_import("file_writelines", 3, &mut self.import_ids);
        add_import("file_seek", 5, &mut self.import_ids);
        add_import("file_tell", 2, &mut self.import_ids);
        add_import("file_fileno", 2, &mut self.import_ids);
        add_import("file_truncate", 3, &mut self.import_ids);
        add_import("file_flush", 2, &mut self.import_ids);
        add_import("file_readable", 2, &mut self.import_ids);
        add_import("file_writable", 2, &mut self.import_ids);
        add_import("file_seekable", 2, &mut self.import_ids);
        add_import("file_isatty", 2, &mut self.import_ids);
        add_import("file_close", 2, &mut self.import_ids);
        add_import("file_detach", 2, &mut self.import_ids);
        add_import("file_reconfigure", 9, &mut self.import_ids);

        let reloc_enabled = self.options.reloc_enabled;

        let mut max_func_arity = 0usize;
        let mut max_call_arity = 0usize;
        let mut builtin_trampoline_specs: HashMap<String, usize> = HashMap::new();
        for func_ir in &ir.functions {
            let is_poll = func_ir.name.ends_with("_poll");
            if !is_poll {
                max_func_arity = max_func_arity.max(func_ir.params.len());
            }
            for op in &func_ir.ops {
                if !is_poll
                    && (op.kind == "call_func" || op.kind == "invoke_ffi")
                    && let Some(args) = &op.args
                    && !args.is_empty()
                {
                    max_call_arity = max_call_arity.max(args.len() - 1);
                }
                if op.kind == "builtin_func"
                    && let Some(name) = op.s_value.as_ref()
                {
                    let arity = op.value.unwrap_or(0) as usize;
                    if let Some(prev) = builtin_trampoline_specs.get(name) {
                        if *prev != arity {
                            panic!(
                                "builtin trampoline arity mismatch for {name}: {prev} vs {arity}"
                            );
                        }
                    } else {
                        builtin_trampoline_specs.insert(name.clone(), arity);
                    }
                }
            }
        }
        let mut auto_import_names: Vec<(String, usize)> = builtin_trampoline_specs
            .iter()
            .map(|(runtime_name, arity)| {
                (
                    runtime_name
                        .strip_prefix("molt_")
                        .unwrap_or(runtime_name.as_str())
                        .to_string(),
                    *arity,
                )
            })
            .filter(|(import_name, _)| !self.import_ids.contains_key(import_name))
            .collect();
        auto_import_names.sort_by(|a, b| a.0.cmp(&b.0));
        for (import_name, arity) in auto_import_names {
            add_import(
                import_name.as_str(),
                simple_i64_import_type(arity),
                &mut self.import_ids,
            );
        }
        self.func_count = import_idx;
        // skipped_indices not used in the "skip entirely" approach,
        // but preserved on the struct for future emit_call_or_unreachable use.
        self.skipped_import_indices = skipped_indices;

        let mut user_type_map: HashMap<usize, u32> = HashMap::new();
        // Types 0-34 are defined above (0-30 single-return, 31-34 multi-value);
        // start new dynamic signatures after them.
        let mut next_type_idx = STATIC_TYPE_COUNT;
        for func_ir in &ir.functions {
            if func_ir.name.ends_with("_poll") {
                continue;
            }
            let arity = func_ir.params.len();
            if let std::collections::hash_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        // Multi-value return type signatures for candidate functions.
        // Maps (param_count, return_count) -> type index.
        let mut multi_return_type_map: HashMap<(usize, usize), u32> = HashMap::new();
        {
            // Collect unique (param_count, return_count) pairs from candidates.
            let func_param_counts: HashMap<&str, usize> = ir
                .functions
                .iter()
                .map(|f| (f.name.as_str(), f.params.len()))
                .collect();
            let mut needed: Vec<(usize, usize)> = Vec::new();
            for (name, ret_count) in &multi_return_candidates {
                if let Some(&param_count) = func_param_counts.get(name.as_str()) {
                    let key = (param_count, *ret_count);
                    if !multi_return_type_map.contains_key(&key) {
                        multi_return_type_map.insert(key, next_type_idx);
                        needed.push(key);
                        next_type_idx += 1;
                    }
                }
            }
            // Sort for deterministic type section ordering.
            needed.sort();
            // Re-assign indices in sorted order.
            let base = next_type_idx - needed.len() as u32;
            for (i, key) in needed.iter().enumerate() {
                multi_return_type_map.insert(*key, base + i as u32);
            }
            for (param_count, ret_count) in &needed {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, *param_count),
                    std::iter::repeat_n(ValType::I64, *ret_count),
                );
            }
        }

        let max_call_indirect = 13usize;
        let max_needed_arity = max_func_arity
            .max(max_call_arity.saturating_add(3))
            .max(max_call_indirect + 1);
        for arity in 0..=max_needed_arity {
            if let std::collections::hash_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        let mut call_indirect_type_map: HashMap<usize, u32> = HashMap::new();
        for arity in 0..=max_call_indirect + 1 {
            self.types.function(
                std::iter::repeat_n(ValType::I64, arity),
                std::iter::once(ValType::I64),
            );
            call_indirect_type_map.insert(arity, next_type_idx);
            next_type_idx += 1;
        }

        for arity in 0..=max_call_indirect {
            let sig_idx = *call_indirect_type_map.get(&(arity + 1)).unwrap_or_else(|| {
                panic!("missing call_indirect signature for arity {}", arity + 1)
            });
            let callee_idx = *call_indirect_type_map
                .get(&arity)
                .unwrap_or_else(|| panic!("missing call_indirect callee type for arity {}", arity));
            self.funcs.function(sig_idx);
            let export_name = format!("molt_call_indirect{arity}");
            self.exports
                .export(&export_name, ExportKind::Func, self.func_count);
            let mut call_indirect = Function::new_with_locals_types(Vec::new());
            for idx in 0..arity {
                call_indirect.instruction(&Instruction::LocalGet((idx + 1) as u32));
            }
            call_indirect.instruction(&Instruction::LocalGet(0));
            call_indirect.instruction(&Instruction::I32WrapI64);
            emit_call_indirect(&mut call_indirect, reloc_enabled, callee_idx, 0);
            call_indirect.instruction(&Instruction::End);
            self.codes.function(&call_indirect);
            self.func_count += 1;
        }

        let sentinel_func_idx = self.func_count;
        self.funcs.function(2);
        let mut sentinel = Function::new_with_locals_types(Vec::new());
        sentinel.instruction(&Instruction::I64Const(0));
        sentinel.instruction(&Instruction::End);
        self.codes.function(&sentinel);
        self.func_count += 1;

        // Memory & Table (imported for shared-instance linking)

        let mut builtin_table_funcs: Vec<(&str, &str, usize)> = vec![
            ("molt_missing", "missing", 0),
            ("molt_pending", "pending", 0),
            ("molt_repr_builtin", "repr_builtin", 1),
            ("molt_format_builtin", "format_builtin", 2),
            ("molt_callable_builtin", "callable_builtin", 1),
            ("molt_round_builtin", "round_builtin", 2),
            ("molt_enumerate_builtin", "enumerate_builtin", 2),
            ("molt_iter_sentinel", "iter_sentinel", 2),
            ("molt_next_builtin", "next_builtin", 2),
            ("molt_any_builtin", "any_builtin", 1),
            ("molt_all_builtin", "all_builtin", 1),
            ("molt_sum_builtin", "sum_builtin", 2),
            ("molt_min_builtin", "min_builtin", 3),
            ("molt_max_builtin", "max_builtin", 3),
            ("molt_sorted_builtin", "sorted_builtin", 3),
            ("molt_map_builtin", "map_builtin", 2),
            ("molt_filter_builtin", "filter_builtin", 2),
            ("molt_zip_builtin", "zip_builtin", 2),
            ("molt_reversed_builtin", "reversed_builtin", 1),
            ("molt_getattr_builtin", "getattr_builtin", 3),
            ("molt_dir_builtin", "dir_builtin", 1),
            ("molt_vars_builtin", "vars_builtin", 1),
            ("molt_anext_builtin", "anext_builtin", 2),
            ("molt_print_builtin", "print_builtin", 5),
            ("molt_super_builtin", "super_builtin", 2),
            ("molt_set_attr_name", "set_attr_name", 3),
            ("molt_del_attr_name", "del_attr_name", 2),
            ("molt_has_attr_name", "has_attr_name", 2),
            ("molt_isinstance", "isinstance", 2),
            ("molt_issubclass", "issubclass", 2),
            ("molt_len", "len", 1),
            ("molt_id", "id", 1),
            ("molt_hash_builtin", "hash_builtin", 1),
            ("molt_ord", "ord", 1),
            ("molt_chr", "chr", 1),
            ("molt_ascii_from_obj", "ascii_from_obj", 1),
            ("molt_bin_builtin", "bin_builtin", 1),
            ("molt_oct_builtin", "oct_builtin", 1),
            ("molt_hex_builtin", "hex_builtin", 1),
            ("molt_abs_builtin", "abs_builtin", 1),
            ("molt_divmod_builtin", "divmod_builtin", 2),
            ("molt_open_builtin", "open_builtin", 8),
            ("molt_getargv", "getargv", 0),
            ("molt_getframe", "getframe", 1),
            ("molt_trace_enter_slot", "trace_enter_slot", 1),
            ("molt_trace_set_line", "trace_set_line", 1),
            ("molt_trace_exit", "trace_exit", 0),
            ("molt_sys_version_info", "sys_version_info", 0),
            ("molt_sys_version", "sys_version", 0),
            ("molt_sys_hexversion", "sys_hexversion", 0),
            ("molt_sys_api_version", "sys_api_version", 0),
            ("molt_sys_abiflags", "sys_abiflags", 0),
            (
                "molt_sys_implementation_payload",
                "sys_implementation_payload",
                0,
            ),
            ("molt_sys_stdin", "sys_stdin", 0),
            ("molt_sys_stdout", "sys_stdout", 0),
            ("molt_sys_stderr", "sys_stderr", 0),
            ("molt_sys_executable", "sys_executable", 0),
            ("molt_sys_set_version_info", "sys_set_version_info", 6),
            ("molt_env_get", "env_get", 2),
            ("molt_env_snapshot", "env_snapshot", 0),
            ("molt_os_name", "os_name", 0),
            ("molt_os_close", "os_close", 1),
            ("molt_os_dup", "os_dup", 1),
            ("molt_os_get_inheritable", "os_get_inheritable", 1),
            ("molt_os_set_inheritable", "os_set_inheritable", 2),
            ("molt_os_urandom", "os_urandom", 1),
            ("molt_sys_platform", "sys_platform", 0),
            ("molt_errno_constants", "errno_constants", 0),
            ("molt_socket_constants", "socket_constants", 0),
            ("molt_socket_has_ipv6", "socket_has_ipv6", 0),
            ("molt_socket_new", "socket_new", 4),
            ("molt_socket_close", "socket_close", 1),
            ("molt_socket_drop", "socket_drop", 1),
            ("molt_socket_clone", "socket_clone", 1),
            ("molt_socket_fileno", "socket_fileno", 1),
            ("molt_socket_gettimeout", "socket_gettimeout", 1),
            ("molt_socket_settimeout", "socket_settimeout", 2),
            ("molt_socket_setblocking", "socket_setblocking", 2),
            ("molt_socket_getblocking", "socket_getblocking", 1),
            ("molt_socket_bind", "socket_bind", 2),
            ("molt_socket_listen", "socket_listen", 2),
            ("molt_socket_accept", "socket_accept", 1),
            ("molt_socket_connect", "socket_connect", 2),
            ("molt_socket_connect_ex", "socket_connect_ex", 2),
            ("molt_socket_recv", "socket_recv", 3),
            ("molt_socket_recv_into", "socket_recv_into", 4),
            ("molt_socket_send", "socket_send", 3),
            ("molt_socket_sendall", "socket_sendall", 3),
            ("molt_socket_sendto", "socket_sendto", 4),
            ("molt_socket_recvfrom", "socket_recvfrom", 3),
            ("molt_socket_shutdown", "socket_shutdown", 2),
            ("molt_socket_getsockname", "socket_getsockname", 1),
            ("molt_socket_getpeername", "socket_getpeername", 1),
            ("molt_socket_setsockopt", "socket_setsockopt", 4),
            ("molt_socket_getsockopt", "socket_getsockopt", 4),
            ("molt_socket_detach", "socket_detach", 1),
            ("molt_socketpair", "socketpair", 3),
            ("molt_socket_getaddrinfo", "socket_getaddrinfo", 6),
            ("molt_socket_getnameinfo", "socket_getnameinfo", 2),
            ("molt_socket_gethostname", "socket_gethostname", 0),
            ("molt_socket_getservbyname", "socket_getservbyname", 2),
            ("molt_socket_getservbyport", "socket_getservbyport", 2),
            ("molt_socket_inet_pton", "socket_inet_pton", 2),
            ("molt_socket_inet_ntop", "socket_inet_ntop", 2),
            ("molt_getpid", "getpid", 0),
            ("molt_getcwd", "getcwd", 0),
            ("molt_time_monotonic", "time_monotonic", 0),
            ("molt_time_monotonic_ns", "time_monotonic_ns", 0),
            ("molt_time_perf_counter", "time_perf_counter", 0),
            ("molt_time_perf_counter_ns", "time_perf_counter_ns", 0),
            ("molt_time_process_time", "time_process_time", 0),
            ("molt_time_process_time_ns", "time_process_time_ns", 0),
            ("molt_time_time", "time_time", 0),
            ("molt_time_time_ns", "time_time_ns", 0),
            ("molt_time_localtime", "time_localtime", 1),
            ("molt_time_gmtime", "time_gmtime", 1),
            ("molt_time_strftime", "time_strftime", 2),
            ("molt_time_timezone", "time_timezone", 0),
            ("molt_time_tzname", "time_tzname", 0),
            ("molt_math_log", "math_log", 1),
            ("molt_math_log2", "math_log2", 1),
            ("molt_math_exp", "math_exp", 1),
            ("molt_math_sin", "math_sin", 1),
            ("molt_math_cos", "math_cos", 1),
            ("molt_math_acos", "math_acos", 1),
            ("molt_math_lgamma", "math_lgamma", 1),
            ("molt_path_exists", "path_exists", 1),
            ("molt_path_listdir", "path_listdir", 1),
            ("molt_path_mkdir", "path_mkdir", 2),
            ("molt_path_unlink", "path_unlink", 1),
            ("molt_path_rmdir", "path_rmdir", 1),
            ("molt_path_chmod", "path_chmod", 2),
            ("molt_getrecursionlimit", "getrecursionlimit", 0),
            ("molt_setrecursionlimit", "setrecursionlimit", 1),
            ("molt_site_help0", "site_help0", 0),
            ("molt_site_help1", "site_help1", 1),
            ("molt_future_features", "future_features", 0),
            ("molt_exception_last", "exception_last", 0),
            ("molt_exception_active", "exception_active", 0),
            ("molt_asyncgen_hooks_get", "asyncgen_hooks_get", 0),
            ("molt_asyncgen_hooks_set", "asyncgen_hooks_set", 2),
            ("molt_asyncgen_locals", "asyncgen_locals", 1),
            ("molt_gen_locals", "gen_locals", 1),
            ("molt_asyncgen_shutdown", "asyncgen_shutdown", 0),
            ("molt_code_new", "code_new", 8),
            ("molt_compile_builtin", "compile_builtin", 6),
            ("molt_module_new", "module_new", 1),
            ("molt_module_import", "module_import", 1),
            ("molt_module_cache_set", "module_cache_set", 2),
            ("molt_class_new", "class_new", 1),
            ("molt_class_set_base", "class_set_base", 2),
            ("molt_class_apply_set_name", "class_apply_set_name", 1),
            ("molt_function_set_builtin", "function_set_builtin", 1),
            ("molt_exceptiongroup_match", "exceptiongroup_match", 2),
            ("molt_exceptiongroup_combine", "exceptiongroup_combine", 1),
            ("molt_iter_checked", "iter", 1),
            ("molt_aiter", "aiter", 1),
            ("molt_io_wait_new", "io_wait_new", 3),
            ("molt_ws_wait_new", "ws_wait_new", 3),
            ("molt_ws_pair_obj", "ws_pair_obj", 1),
            ("molt_ws_connect_obj", "ws_connect_obj", 1),
            ("molt_ws_send_obj", "ws_send_obj", 2),
            ("molt_ws_recv", "ws_recv", 1),
            ("molt_ws_close", "ws_close", 1),
            ("molt_ws_drop", "ws_drop", 1),
            ("molt_future_cancel", "future_cancel", 1),
            ("molt_future_cancel_msg", "future_cancel_msg", 2),
            ("molt_future_cancel_clear", "future_cancel_clear", 1),
            ("molt_block_on", "block_on", 1),
            ("molt_lock_new", "lock_new", 0),
            ("molt_lock_acquire", "lock_acquire", 3),
            ("molt_lock_release", "lock_release", 1),
            ("molt_lock_locked", "lock_locked", 1),
            ("molt_lock_drop", "lock_drop", 1),
            ("molt_rlock_new", "rlock_new", 0),
            ("molt_rlock_acquire", "rlock_acquire", 3),
            ("molt_rlock_release", "rlock_release", 1),
            ("molt_rlock_locked", "rlock_locked", 1),
            ("molt_rlock_drop", "rlock_drop", 1),
            ("molt_chan_new", "chan_new", 1),
            ("molt_chan_send", "chan_send", 2),
            ("molt_chan_send_blocking", "chan_send_blocking", 2),
            ("molt_chan_try_send", "chan_try_send", 2),
            ("molt_chan_recv", "chan_recv", 1),
            ("molt_chan_recv_blocking", "chan_recv_blocking", 1),
            ("molt_chan_try_recv", "chan_try_recv", 1),
            ("molt_heapq_heapify", "heapq_heapify", 1),
            ("molt_heapq_heappush", "heapq_heappush", 2),
            ("molt_heapq_heappop", "heapq_heappop", 1),
            ("molt_heapq_heapreplace", "heapq_heapreplace", 2),
            ("molt_heapq_heappushpop", "heapq_heappushpop", 2),
            ("molt_struct_calcsize", "struct_calcsize", 1),
            ("molt_struct_pack", "struct_pack", 2),
            ("molt_struct_unpack", "struct_unpack", 2),
            ("molt_struct_pack_into", "struct_pack_into", 3),
            ("molt_struct_unpack_from", "struct_unpack_from", 3),
            ("molt_struct_iter_unpack", "struct_iter_unpack", 2),
            ("molt_thread_spawn", "thread_spawn", 1),
            ("molt_thread_join", "thread_join", 2),
            ("molt_thread_is_alive", "thread_is_alive", 1),
            ("molt_thread_ident", "thread_ident", 1),
            ("molt_thread_native_id", "thread_native_id", 1),
            ("molt_thread_current_ident", "thread_current_ident", 0),
            (
                "molt_thread_current_native_id",
                "thread_current_native_id",
                0,
            ),
            ("molt_thread_drop", "thread_drop", 1),
            ("molt_process_spawn", "process_spawn", 6),
            ("molt_process_wait_future", "process_wait_future", 1),
            ("molt_process_pid", "process_pid", 1),
            ("molt_process_returncode", "process_returncode", 1),
            ("molt_process_kill", "process_kill", 1),
            ("molt_process_terminate", "process_terminate", 1),
            ("molt_process_stdin", "process_stdin", 1),
            ("molt_process_stdout", "process_stdout", 1),
            ("molt_process_stderr", "process_stderr", 1),
            ("molt_process_drop", "process_drop", 1),
            ("molt_stream_new", "stream_new", 1),
            ("molt_stream_clone", "stream_clone", 1),
            ("molt_stream_send_obj", "stream_send_obj", 2),
            ("molt_stream_recv", "stream_recv", 1),
            ("molt_stream_close", "stream_close", 1),
            ("molt_stream_drop", "stream_drop", 1),
            ("molt_weakref_register", "weakref_register", 3),
            ("molt_weakref_get", "weakref_get", 1),
            ("molt_weakref_drop", "weakref_drop", 1),
        ];
        builtin_table_funcs.extend([
            (
                "molt_importlib_bootstrap_payload",
                "importlib_bootstrap_payload",
                2,
            ),
            (
                "molt_importlib_cache_from_source",
                "importlib_cache_from_source",
                1,
            ),
            (
                "molt_importlib_coerce_module_name",
                "importlib_coerce_module_name",
                3,
            ),
            ("molt_importlib_decode_source", "importlib_decode_source", 1),
            (
                "molt_importlib_ensure_default_meta_path",
                "importlib_ensure_default_meta_path",
                1,
            ),
            (
                "molt_importlib_exec_extension",
                "importlib_exec_extension",
                3,
            ),
            (
                "molt_importlib_exec_restricted_source",
                "importlib_exec_restricted_source",
                3,
            ),
            (
                "molt_importlib_exec_sourceless",
                "importlib_exec_sourceless",
                3,
            ),
            (
                "molt_importlib_extension_loader_payload",
                "importlib_extension_loader_payload",
                3,
            ),
            (
                "molt_importlib_filefinder_find_spec",
                "importlib_filefinder_find_spec",
                3,
            ),
            (
                "molt_importlib_filefinder_invalidate",
                "importlib_filefinder_invalidate",
                1,
            ),
            ("molt_importlib_find_in_path", "importlib_find_in_path", 2),
            (
                "molt_importlib_find_in_path_package_context",
                "importlib_find_in_path_package_context",
                2,
            ),
            ("molt_importlib_find_spec", "importlib_find_spec", 8),
            (
                "molt_importlib_find_spec_orchestrate",
                "importlib_find_spec_orchestrate",
                5,
            ),
            (
                "molt_importlib_frozen_external_payload",
                "importlib_frozen_external_payload",
                2,
            ),
            (
                "molt_importlib_frozen_payload",
                "importlib_frozen_payload",
                2,
            ),
            ("molt_importlib_import_module", "importlib_import_module", 3),
            (
                "molt_importlib_import_optional",
                "importlib_import_optional",
                1,
            ),
            (
                "molt_importlib_import_or_fallback",
                "importlib_import_or_fallback",
                2,
            ),
            (
                "molt_importlib_import_required",
                "importlib_import_required",
                1,
            ),
            (
                "molt_importlib_invalidate_caches",
                "importlib_invalidate_caches",
                0,
            ),
            (
                "molt_importlib_known_absent_missing_name",
                "importlib_known_absent_missing_name",
                1,
            ),
            (
                "molt_importlib_load_module_shim",
                "importlib_load_module_shim",
                3,
            ),
            (
                "molt_importlib_metadata_dist_paths",
                "importlib_metadata_dist_paths",
                2,
            ),
            (
                "molt_importlib_metadata_distributions_payload",
                "importlib_metadata_distributions_payload",
                2,
            ),
            (
                "molt_importlib_metadata_entry_points_filter_payload",
                "importlib_metadata_entry_points_filter_payload",
                5,
            ),
            (
                "molt_importlib_metadata_entry_points_select_payload",
                "importlib_metadata_entry_points_select_payload",
                4,
            ),
            (
                "molt_importlib_metadata_normalize_name",
                "importlib_metadata_normalize_name",
                1,
            ),
            (
                "molt_importlib_metadata_packages_distributions_payload",
                "importlib_metadata_packages_distributions_payload",
                2,
            ),
            (
                "molt_importlib_metadata_payload",
                "importlib_metadata_payload",
                1,
            ),
            (
                "molt_importlib_metadata_record_payload",
                "importlib_metadata_record_payload",
                1,
            ),
            (
                "molt_importlib_metadata_types_payload",
                "importlib_metadata_types_payload",
                4,
            ),
            (
                "molt_importlib_module_from_spec",
                "importlib_module_from_spec",
                1,
            ),
            (
                "molt_importlib_module_spec_is_package",
                "importlib_module_spec_is_package",
                1,
            ),
            (
                "molt_importlib_package_root_from_origin",
                "importlib_package_root_from_origin",
                1,
            ),
            (
                "molt_importlib_path_is_archive_member",
                "importlib_path_is_archive_member",
                1,
            ),
            (
                "molt_importlib_pathfinder_find_spec",
                "importlib_pathfinder_find_spec",
                3,
            ),
            ("molt_importlib_read_file", "importlib_read_file", 1),
            ("molt_importlib_reload", "importlib_reload", 4),
            ("molt_importlib_resolve_name", "importlib_resolve_name", 2),
            (
                "molt_importlib_resources_as_file_enter",
                "importlib_resources_as_file_enter",
                2,
            ),
            (
                "molt_importlib_resources_as_file_exit",
                "importlib_resources_as_file_exit",
                3,
            ),
            (
                "molt_importlib_resources_contents_from_package",
                "importlib_resources_contents_from_package",
                3,
            ),
            (
                "molt_importlib_resources_contents_from_package_parts",
                "importlib_resources_contents_from_package_parts",
                4,
            ),
            (
                "molt_importlib_resources_files_payload",
                "importlib_resources_files_payload",
                4,
            ),
            (
                "molt_importlib_resources_is_resource_from_package",
                "importlib_resources_is_resource_from_package",
                4,
            ),
            (
                "molt_importlib_resources_is_resource_from_package_parts",
                "importlib_resources_is_resource_from_package_parts",
                4,
            ),
            (
                "molt_importlib_resources_joinpath",
                "importlib_resources_joinpath",
                2,
            ),
            (
                "molt_importlib_resources_loader_reader",
                "importlib_resources_loader_reader",
                2,
            ),
            (
                "molt_importlib_resources_module_name",
                "importlib_resources_module_name",
                2,
            ),
            (
                "molt_importlib_resources_normalize_path",
                "importlib_resources_normalize_path",
                1,
            ),
            (
                "molt_importlib_resources_only",
                "importlib_resources_only",
                3,
            ),
            (
                "molt_importlib_resources_open_mode_is_text",
                "importlib_resources_open_mode_is_text",
                1,
            ),
            (
                "molt_importlib_resources_open_resource_bytes_from_package",
                "importlib_resources_open_resource_bytes_from_package",
                4,
            ),
            (
                "molt_importlib_resources_open_resource_bytes_from_package_parts",
                "importlib_resources_open_resource_bytes_from_package_parts",
                4,
            ),
            (
                "molt_importlib_resources_package_info",
                "importlib_resources_package_info",
                3,
            ),
            (
                "molt_importlib_resources_package_leaf_name",
                "importlib_resources_package_leaf_name",
                1,
            ),
            (
                "molt_importlib_resources_path_payload",
                "importlib_resources_path_payload",
                1,
            ),
            (
                "molt_importlib_resources_read_text_from_package",
                "importlib_resources_read_text_from_package",
                6,
            ),
            (
                "molt_importlib_resources_read_text_from_package_parts",
                "importlib_resources_read_text_from_package_parts",
                6,
            ),
            (
                "molt_importlib_resources_reader_child_names",
                "importlib_resources_reader_child_names",
                2,
            ),
            (
                "molt_importlib_resources_reader_contents",
                "importlib_resources_reader_contents",
                1,
            ),
            (
                "molt_importlib_resources_reader_contents_from_roots",
                "importlib_resources_reader_contents_from_roots",
                1,
            ),
            (
                "molt_importlib_resources_reader_exists",
                "importlib_resources_reader_exists",
                2,
            ),
            (
                "molt_importlib_resources_reader_files_traversable",
                "importlib_resources_reader_files_traversable",
                1,
            ),
            (
                "molt_importlib_resources_reader_is_dir",
                "importlib_resources_reader_is_dir",
                2,
            ),
            (
                "molt_importlib_resources_reader_is_resource",
                "importlib_resources_reader_is_resource",
                2,
            ),
            (
                "molt_importlib_resources_reader_is_resource_from_roots",
                "importlib_resources_reader_is_resource_from_roots",
                2,
            ),
            (
                "molt_importlib_resources_reader_open_resource_bytes",
                "importlib_resources_reader_open_resource_bytes",
                2,
            ),
            (
                "molt_importlib_resources_reader_open_resource_bytes_from_roots",
                "importlib_resources_reader_open_resource_bytes_from_roots",
                2,
            ),
            (
                "molt_importlib_resources_reader_resource_path",
                "importlib_resources_reader_resource_path",
                2,
            ),
            (
                "molt_importlib_resources_reader_resource_path_from_roots",
                "importlib_resources_reader_resource_path_from_roots",
                2,
            ),
            (
                "molt_importlib_resources_reader_roots",
                "importlib_resources_reader_roots",
                1,
            ),
            (
                "molt_importlib_resources_resource_path_from_package",
                "importlib_resources_resource_path_from_package",
                4,
            ),
            (
                "molt_importlib_resources_resource_path_from_package_parts",
                "importlib_resources_resource_path_from_package_parts",
                4,
            ),
            (
                "molt_importlib_runtime_modules",
                "importlib_runtime_modules",
                0,
            ),
            (
                "molt_importlib_set_module_state",
                "importlib_set_module_state",
                8,
            ),
            (
                "molt_importlib_source_exec_payload",
                "importlib_source_exec_payload",
                3,
            ),
            (
                "molt_importlib_source_from_cache",
                "importlib_source_from_cache",
                1,
            ),
            ("molt_importlib_source_hash", "importlib_source_hash", 1),
            (
                "molt_importlib_sourceless_loader_payload",
                "importlib_sourceless_loader_payload",
                3,
            ),
            (
                "molt_importlib_spec_from_file_location",
                "importlib_spec_from_file_location",
                5,
            ),
            (
                "molt_importlib_spec_from_loader",
                "importlib_spec_from_loader",
                5,
            ),
            (
                "molt_importlib_stabilize_module_state",
                "importlib_stabilize_module_state",
                6,
            ),
            (
                "molt_importlib_validate_resource_name",
                "importlib_validate_resource_name",
                1,
            ),
            (
                "molt_importlib_zip_read_entry",
                "importlib_zip_read_entry",
                2,
            ),
            (
                "molt_importlib_zip_source_exec_payload",
                "importlib_zip_source_exec_payload",
                4,
            ),
            ("molt_os_access", "os_access", 2),
            ("molt_os_altsep", "os_altsep", 0),
            ("molt_os_chdir", "os_chdir", 1),
            ("molt_os_chmod", "os_chmod", 2),
            ("molt_os_cpu_count", "os_cpu_count", 0),
            ("molt_os_curdir", "os_curdir", 0),
            ("molt_os_devnull", "os_devnull", 0),
            ("molt_os_dup2", "os_dup2", 2),
            ("molt_os_extsep", "os_extsep", 0),
            ("molt_os_fdopen", "os_fdopen", 3),
            ("molt_os_fsencode", "os_fsencode", 1),
            ("molt_os_fspath", "os_fspath", 1),
            ("molt_os_fstat", "os_fstat", 1),
            ("molt_os_ftruncate", "os_ftruncate", 2),
            ("molt_os_get_terminal_size", "os_get_terminal_size", 1),
            ("molt_os_getcwd", "os_getcwd", 0),
            ("molt_os_getegid", "os_getegid", 0),
            ("molt_os_geteuid", "os_geteuid", 0),
            ("molt_os_getgid", "os_getgid", 0),
            ("molt_os_getloadavg", "os_getloadavg", 0),
            ("molt_os_getlogin", "os_getlogin", 0),
            ("molt_os_getpgrp", "os_getpgrp", 0),
            ("molt_os_getpid", "os_getpid", 0),
            ("molt_os_getppid", "os_getppid", 0),
            ("molt_os_getuid", "os_getuid", 0),
            ("molt_os_isatty", "os_isatty", 1),
            ("molt_os_kill", "os_kill", 2),
            ("molt_os_linesep", "os_linesep", 0),
            ("molt_os_link", "os_link", 2),
            ("molt_os_listdir", "os_listdir", 1),
            ("molt_os_lseek", "os_lseek", 3),
            ("molt_os_lstat", "os_lstat", 1),
            ("molt_os_mkdir", "os_mkdir", 2),
            ("molt_os_open", "os_open", 3),
            ("molt_os_open_flags", "os_open_flags", 0),
            ("molt_os_pardir", "os_pardir", 0),
            ("molt_os_path_commonpath", "os_path_commonpath", 1),
            ("molt_os_path_commonprefix", "os_path_commonprefix", 1),
            ("molt_os_path_getatime", "os_path_getatime", 1),
            ("molt_os_path_getctime", "os_path_getctime", 1),
            ("molt_os_path_getmtime", "os_path_getmtime", 1),
            ("molt_os_path_getsize", "os_path_getsize", 1),
            ("molt_os_path_realpath", "os_path_realpath", 1),
            ("molt_os_path_samefile", "os_path_samefile", 2),
            ("molt_os_pathsep", "os_pathsep", 0),
            ("molt_os_pipe", "os_pipe", 0),
            ("molt_os_read", "os_read", 2),
            ("molt_os_readlink", "os_readlink", 1),
            ("molt_os_removedirs", "os_removedirs", 1),
            ("molt_os_rename", "os_rename", 2),
            ("molt_os_replace", "os_replace", 2),
            ("molt_os_rmdir", "os_rmdir", 1),
            ("molt_os_scandir", "os_scandir", 1),
            ("molt_os_sendfile", "os_sendfile", 4),
            ("molt_os_sep", "os_sep", 0),
            ("molt_os_setpgrp", "os_setpgrp", 0),
            ("molt_os_setsid", "os_setsid", 0),
            ("molt_os_stat", "os_stat", 1),
            ("molt_os_symlink", "os_symlink", 2),
            ("molt_os_sysconf", "os_sysconf", 1),
            ("molt_os_sysconf_names", "os_sysconf_names", 0),
            ("molt_os_truncate", "os_truncate", 2),
            ("molt_os_umask", "os_umask", 1),
            ("molt_os_uname", "os_uname", 0),
            ("molt_os_utime", "os_utime", 3),
            ("molt_os_waitpid", "os_waitpid", 2),
            ("molt_os_walk", "os_walk", 3),
            ("molt_os_wexitstatus", "os_wexitstatus", 1),
            ("molt_os_wifexited", "os_wifexited", 1),
            ("molt_os_wifsignaled", "os_wifsignaled", 1),
            ("molt_os_wifstopped", "os_wifstopped", 1),
            ("molt_os_write", "os_write", 2),
            ("molt_os_wstopsig", "os_wstopsig", 1),
            ("molt_os_wtermsig", "os_wtermsig", 1),
        ]);
        let hardcoded_builtin_runtime_names: HashSet<&str> = builtin_table_funcs
            .iter()
            .map(|(runtime_name, _, _)| *runtime_name)
            .collect();
        let mut auto_builtin_table_funcs: Vec<(String, String, usize)> = builtin_trampoline_specs
            .iter()
            .filter(|(runtime_name, _)| {
                !hardcoded_builtin_runtime_names.contains(runtime_name.as_str())
            })
            .map(|(runtime_name, arity)| {
                let import_name = runtime_name
                    .strip_prefix("molt_")
                    .unwrap_or(runtime_name.as_str())
                    .to_string();
                (runtime_name.clone(), import_name, *arity)
            })
            .collect();
        auto_builtin_table_funcs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut builtin_trampoline_funcs: Vec<(String, usize)> = Vec::new();
        let builtin_runtime_names: HashSet<&str> = builtin_table_funcs
            .iter()
            .map(|(runtime_name, _, _)| *runtime_name)
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, _, _)| runtime_name.as_str()),
            )
            .collect();
        for runtime_name in builtin_table_funcs
            .iter()
            .map(|(runtime_name, _, _)| *runtime_name)
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, _, _)| runtime_name.as_str()),
            )
        {
            if let Some(arity) = builtin_trampoline_specs.get(runtime_name) {
                builtin_trampoline_funcs.push((runtime_name.to_string(), *arity));
            }
        }
        // Intrinsic ABIs are canonicalized to i64/u64 for dynamic function-object dispatch.
        // Keep wrapper conversion sets empty so generated wrappers preserve 64-bit bits values.
        let builtin_i32_arg0_imports: HashSet<&str> = [].into_iter().collect();
        let builtin_i32_return_imports: HashSet<&str> = [].into_iter().collect();
        let void_builtin_imports: HashSet<&str> = [
            "process_drop",
            "socket_drop",
            "stream_close",
            "stream_drop",
            "ws_close",
            "ws_drop",
        ]
        .into_iter()
        .collect();
        let mut builtin_wrapper_funcs: Vec<(String, String, usize)> = Vec::new();
        let wrap_all_builtins = reloc_enabled;
        for (runtime_name, import_name, arity) in builtin_table_funcs
            .iter()
            .map(|(runtime_name, import_name, arity)| {
                (
                    (*runtime_name).to_string(),
                    (*import_name).to_string(),
                    *arity,
                )
            })
            .chain(auto_builtin_table_funcs.iter().cloned())
        {
            if wrap_all_builtins
                || builtin_trampoline_specs.contains_key(runtime_name.as_str())
                || void_builtin_imports.contains(import_name.as_str())
                || builtin_i32_arg0_imports.contains(import_name.as_str())
                || builtin_i32_return_imports.contains(import_name.as_str())
            {
                builtin_wrapper_funcs.push((runtime_name, import_name, arity));
            }
        }
        if builtin_trampoline_specs.len() != builtin_trampoline_funcs.len() {
            for name in builtin_trampoline_specs.keys() {
                if !builtin_runtime_names.contains(name.as_str()) {
                    panic!("builtin {name} missing from wasm table");
                }
            }
        }
        let builtin_table_len = builtin_table_funcs.len() + auto_builtin_table_funcs.len();
        let table_base: u32 = self.options.table_base;
        let poll_table_prefix = 33u32;
        let table_len = (poll_table_prefix as usize
            + builtin_table_len
            + builtin_trampoline_funcs.len()
            + ir.functions.len() * 2) as u32;
        let table_min = table_base + table_len;
        let table_ty = TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: u64::from(table_min),
            maximum: None,
            shared: false,
        };
        self.imports.import(
            "env",
            "__indirect_function_table",
            EntityType::Table(table_ty),
        );
        self.exports.export("molt_table", ExportKind::Table, 0);

        let mut builtin_wrapper_indices = HashMap::new();
        for (runtime_name, import_name, arity) in &builtin_wrapper_funcs {
            let type_idx = *user_type_map
                .get(arity)
                .unwrap_or_else(|| panic!("missing builtin wrapper signature for arity {arity}"));
            let import_idx = *self
                .import_ids
                .get(import_name.as_str())
                .unwrap_or_else(|| panic!("missing builtin import for {import_name}"));
            self.funcs.function(type_idx);
            let func_index = self.func_count;
            self.func_count += 1;
            let mut func = Function::new_with_locals_types(Vec::new());
            for idx in 0..*arity {
                func.instruction(&Instruction::LocalGet(idx as u32));
                if idx == 0 && builtin_i32_arg0_imports.contains(import_name.as_str()) {
                    func.instruction(&Instruction::I32WrapI64);
                }
            }
            emit_call(&mut func, reloc_enabled, import_idx);
            if builtin_i32_return_imports.contains(import_name.as_str()) {
                func.instruction(&Instruction::I64ExtendI32U);
            }
            if void_builtin_imports.contains(import_name.as_str()) {
                func.instruction(&Instruction::I64Const(box_none()));
            }
            func.instruction(&Instruction::End);
            self.codes.function(&func);
            builtin_wrapper_indices.insert(runtime_name.clone(), func_index);
        }

        let mut table_import_wrappers = HashMap::new();
        if reloc_enabled {
            for (import_name, arity) in [
                ("async_sleep", 1usize),
                ("anext_default_poll", 1usize),
                ("asyncgen_poll", 1usize),
                ("promise_poll", 1usize),
                ("io_wait", 1usize),
                ("thread_poll", 1usize),
                ("process_poll", 1usize),
                ("ws_wait", 1usize),
                ("asyncio_wait_for_poll", 1usize),
                ("asyncio_wait_poll", 1usize),
                ("asyncio_gather_poll", 1usize),
                ("asyncio_socket_reader_read_poll", 1usize),
                ("asyncio_socket_reader_readline_poll", 1usize),
                ("asyncio_stream_reader_read_poll", 1usize),
                ("asyncio_stream_reader_readline_poll", 1usize),
                ("asyncio_stream_send_all_poll", 1usize),
                ("asyncio_sock_recv_poll", 1usize),
                ("asyncio_sock_connect_poll", 1usize),
                ("asyncio_sock_accept_poll", 1usize),
                ("asyncio_sock_recv_into_poll", 1usize),
                ("asyncio_sock_sendall_poll", 1usize),
                ("asyncio_sock_recvfrom_poll", 1usize),
                ("asyncio_sock_recvfrom_into_poll", 1usize),
                ("asyncio_sock_sendto_poll", 1usize),
                ("asyncio_timer_handle_poll", 1usize),
                ("asyncio_fd_watcher_poll", 1usize),
                ("asyncio_server_accept_loop_poll", 1usize),
                ("asyncio_ready_runner_poll", 1usize),
                ("contextlib_asyncgen_enter_poll", 1usize),
                ("contextlib_asyncgen_exit_poll", 1usize),
                ("contextlib_async_exitstack_exit_poll", 1usize),
                ("contextlib_async_exitstack_enter_context_poll", 1usize),
            ] {
                let type_idx = *user_type_map
                    .get(&arity)
                    .unwrap_or_else(|| panic!("missing wrapper signature for arity {arity}"));
                let import_idx = *self
                    .import_ids
                    .get(import_name)
                    .unwrap_or_else(|| panic!("missing import for {import_name}"));
                self.funcs.function(type_idx);
                let func_index = self.func_count;
                self.func_count += 1;
                let mut func = Function::new_with_locals_types(Vec::new());
                for idx in 0..arity {
                    func.instruction(&Instruction::LocalGet(idx as u32));
                }
                emit_call(&mut func, reloc_enabled, import_idx);
                func.instruction(&Instruction::End);
                self.codes.function(&func);
                table_import_wrappers.insert(import_name.to_string(), func_index);
            }
        }

        // Function indices for table
        let async_sleep_idx = *table_import_wrappers
            .get("async_sleep")
            .unwrap_or(&self.import_ids["async_sleep"]);
        let anext_default_poll_idx = *table_import_wrappers
            .get("anext_default_poll")
            .unwrap_or(&self.import_ids["anext_default_poll"]);
        let asyncgen_poll_idx = *table_import_wrappers
            .get("asyncgen_poll")
            .unwrap_or(&self.import_ids["asyncgen_poll"]);
        let promise_poll_idx = *table_import_wrappers
            .get("promise_poll")
            .unwrap_or(&self.import_ids["promise_poll"]);
        let io_wait_idx = *table_import_wrappers
            .get("io_wait")
            .unwrap_or(&self.import_ids["io_wait"]);
        let thread_poll_idx = *table_import_wrappers
            .get("thread_poll")
            .unwrap_or(&self.import_ids["thread_poll"]);
        let process_poll_idx = *table_import_wrappers
            .get("process_poll")
            .unwrap_or(&self.import_ids["process_poll"]);
        let ws_wait_idx = *table_import_wrappers
            .get("ws_wait")
            .unwrap_or(&self.import_ids["ws_wait"]);
        let asyncio_wait_for_poll_idx = *table_import_wrappers
            .get("asyncio_wait_for_poll")
            .unwrap_or(&self.import_ids["asyncio_wait_for_poll"]);
        let asyncio_wait_poll_idx = *table_import_wrappers
            .get("asyncio_wait_poll")
            .unwrap_or(&self.import_ids["asyncio_wait_poll"]);
        let asyncio_gather_poll_idx = *table_import_wrappers
            .get("asyncio_gather_poll")
            .unwrap_or(&self.import_ids["asyncio_gather_poll"]);
        let asyncio_socket_reader_read_poll_idx = *table_import_wrappers
            .get("asyncio_socket_reader_read_poll")
            .unwrap_or(&self.import_ids["asyncio_socket_reader_read_poll"]);
        let asyncio_socket_reader_readline_poll_idx = *table_import_wrappers
            .get("asyncio_socket_reader_readline_poll")
            .unwrap_or(&self.import_ids["asyncio_socket_reader_readline_poll"]);
        let asyncio_stream_reader_read_poll_idx = *table_import_wrappers
            .get("asyncio_stream_reader_read_poll")
            .unwrap_or(&self.import_ids["asyncio_stream_reader_read_poll"]);
        let asyncio_stream_reader_readline_poll_idx = *table_import_wrappers
            .get("asyncio_stream_reader_readline_poll")
            .unwrap_or(&self.import_ids["asyncio_stream_reader_readline_poll"]);
        let asyncio_stream_send_all_poll_idx = *table_import_wrappers
            .get("asyncio_stream_send_all_poll")
            .unwrap_or(&self.import_ids["asyncio_stream_send_all_poll"]);
        let asyncio_sock_recv_poll_idx = *table_import_wrappers
            .get("asyncio_sock_recv_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_recv_poll"]);
        let asyncio_sock_connect_poll_idx = *table_import_wrappers
            .get("asyncio_sock_connect_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_connect_poll"]);
        let asyncio_sock_accept_poll_idx = *table_import_wrappers
            .get("asyncio_sock_accept_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_accept_poll"]);
        let asyncio_sock_recv_into_poll_idx = *table_import_wrappers
            .get("asyncio_sock_recv_into_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_recv_into_poll"]);
        let asyncio_sock_sendall_poll_idx = *table_import_wrappers
            .get("asyncio_sock_sendall_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_sendall_poll"]);
        let asyncio_sock_recvfrom_poll_idx = *table_import_wrappers
            .get("asyncio_sock_recvfrom_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_recvfrom_poll"]);
        let asyncio_sock_recvfrom_into_poll_idx = *table_import_wrappers
            .get("asyncio_sock_recvfrom_into_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_recvfrom_into_poll"]);
        let asyncio_sock_sendto_poll_idx = *table_import_wrappers
            .get("asyncio_sock_sendto_poll")
            .unwrap_or(&self.import_ids["asyncio_sock_sendto_poll"]);
        let asyncio_timer_handle_poll_idx = *table_import_wrappers
            .get("asyncio_timer_handle_poll")
            .unwrap_or(&self.import_ids["asyncio_timer_handle_poll"]);
        let asyncio_fd_watcher_poll_idx = *table_import_wrappers
            .get("asyncio_fd_watcher_poll")
            .unwrap_or(&self.import_ids["asyncio_fd_watcher_poll"]);
        let asyncio_server_accept_loop_poll_idx = *table_import_wrappers
            .get("asyncio_server_accept_loop_poll")
            .unwrap_or(&self.import_ids["asyncio_server_accept_loop_poll"]);
        let asyncio_ready_runner_poll_idx = *table_import_wrappers
            .get("asyncio_ready_runner_poll")
            .unwrap_or(&self.import_ids["asyncio_ready_runner_poll"]);
        let contextlib_asyncgen_enter_poll_idx = *table_import_wrappers
            .get("contextlib_asyncgen_enter_poll")
            .unwrap_or(&self.import_ids["contextlib_asyncgen_enter_poll"]);
        let contextlib_asyncgen_exit_poll_idx = *table_import_wrappers
            .get("contextlib_asyncgen_exit_poll")
            .unwrap_or(&self.import_ids["contextlib_asyncgen_exit_poll"]);
        let contextlib_async_exitstack_exit_poll_idx = *table_import_wrappers
            .get("contextlib_async_exitstack_exit_poll")
            .unwrap_or(&self.import_ids["contextlib_async_exitstack_exit_poll"]);
        let contextlib_async_exitstack_enter_context_poll_idx = *table_import_wrappers
            .get("contextlib_async_exitstack_enter_context_poll")
            .unwrap_or(&self.import_ids["contextlib_async_exitstack_enter_context_poll"]);
        let mut table_indices = vec![
            sentinel_func_idx,
            async_sleep_idx,
            anext_default_poll_idx,
            asyncgen_poll_idx,
            promise_poll_idx,
            io_wait_idx,
            thread_poll_idx,
            process_poll_idx,
            ws_wait_idx,
            asyncio_wait_for_poll_idx,
            asyncio_wait_poll_idx,
            asyncio_gather_poll_idx,
            asyncio_socket_reader_read_poll_idx,
            asyncio_socket_reader_readline_poll_idx,
            asyncio_stream_reader_read_poll_idx,
            asyncio_stream_reader_readline_poll_idx,
            asyncio_stream_send_all_poll_idx,
            asyncio_sock_recv_poll_idx,
            asyncio_sock_connect_poll_idx,
            asyncio_sock_accept_poll_idx,
            asyncio_sock_recv_into_poll_idx,
            asyncio_sock_sendall_poll_idx,
            asyncio_sock_recvfrom_poll_idx,
            asyncio_sock_recvfrom_into_poll_idx,
            asyncio_sock_sendto_poll_idx,
            asyncio_timer_handle_poll_idx,
            asyncio_fd_watcher_poll_idx,
            asyncio_server_accept_loop_poll_idx,
            asyncio_ready_runner_poll_idx,
            contextlib_asyncgen_enter_poll_idx,
            contextlib_asyncgen_exit_poll_idx,
            contextlib_async_exitstack_exit_poll_idx,
            contextlib_async_exitstack_enter_context_poll_idx,
        ];
        let mut func_to_table_idx = HashMap::new();
        let mut func_to_index = HashMap::new();
        func_to_index.insert(
            "molt_runtime_init".to_string(),
            self.import_ids["runtime_init"],
        );
        func_to_index.insert(
            "molt_runtime_shutdown".to_string(),
            self.import_ids["runtime_shutdown"],
        );
        func_to_index.insert(
            "molt_sys_set_version_info".to_string(),
            self.import_ids["sys_set_version_info"],
        );
        func_to_table_idx.insert("molt_async_sleep".to_string(), 1);
        func_to_table_idx.insert("molt_anext_default_poll".to_string(), 2);
        func_to_table_idx.insert("molt_asyncgen_poll".to_string(), 3);
        func_to_table_idx.insert("molt_promise_poll".to_string(), 4);
        func_to_table_idx.insert("molt_io_wait".to_string(), 5);
        func_to_table_idx.insert("molt_thread_poll".to_string(), 6);
        func_to_table_idx.insert("molt_process_poll".to_string(), 7);
        func_to_table_idx.insert("molt_ws_wait".to_string(), 8);
        func_to_table_idx.insert("molt_asyncio_wait_for_poll".to_string(), 9);
        func_to_table_idx.insert("molt_asyncio_wait_poll".to_string(), 10);
        func_to_table_idx.insert("molt_asyncio_gather_poll".to_string(), 11);
        func_to_table_idx.insert("molt_asyncio_socket_reader_read_poll".to_string(), 12);
        func_to_table_idx.insert("molt_asyncio_socket_reader_readline_poll".to_string(), 13);
        func_to_table_idx.insert("molt_asyncio_stream_reader_read_poll".to_string(), 14);
        func_to_table_idx.insert("molt_asyncio_stream_reader_readline_poll".to_string(), 15);
        func_to_table_idx.insert("molt_asyncio_stream_send_all_poll".to_string(), 16);
        func_to_table_idx.insert("molt_asyncio_sock_recv_poll".to_string(), 17);
        func_to_table_idx.insert("molt_asyncio_sock_connect_poll".to_string(), 18);
        func_to_table_idx.insert("molt_asyncio_sock_accept_poll".to_string(), 19);
        func_to_table_idx.insert("molt_asyncio_sock_recv_into_poll".to_string(), 20);
        func_to_table_idx.insert("molt_asyncio_sock_sendall_poll".to_string(), 21);
        func_to_table_idx.insert("molt_asyncio_sock_recvfrom_poll".to_string(), 22);
        func_to_table_idx.insert("molt_asyncio_sock_recvfrom_into_poll".to_string(), 23);
        func_to_table_idx.insert("molt_asyncio_sock_sendto_poll".to_string(), 24);
        func_to_table_idx.insert("molt_asyncio_timer_handle_poll".to_string(), 25);
        func_to_table_idx.insert("molt_asyncio_fd_watcher_poll".to_string(), 26);
        func_to_table_idx.insert("molt_asyncio_server_accept_loop_poll".to_string(), 27);
        func_to_table_idx.insert("molt_asyncio_ready_runner_poll".to_string(), 28);
        func_to_table_idx.insert("molt_contextlib_asyncgen_enter_poll".to_string(), 29);
        func_to_table_idx.insert("molt_contextlib_asyncgen_exit_poll".to_string(), 30);
        func_to_table_idx.insert("molt_contextlib_async_exitstack_exit_poll".to_string(), 31);
        func_to_table_idx.insert(
            "molt_contextlib_async_exitstack_enter_context_poll".to_string(),
            32,
        );

        for (offset, (runtime_name, import_name, _)) in builtin_table_funcs
            .iter()
            .map(|(runtime_name, import_name, arity)| {
                (
                    (*runtime_name).to_string(),
                    (*import_name).to_string(),
                    *arity,
                )
            })
            .chain(auto_builtin_table_funcs.iter().cloned())
            .enumerate()
        {
            let idx = (offset as u32) + poll_table_prefix;
            let runtime_key = runtime_name;
            func_to_table_idx.insert(runtime_key.clone(), idx);
            if let Some(wrapper_idx) = builtin_wrapper_indices.get(&runtime_key) {
                func_to_index.insert(runtime_key, *wrapper_idx);
                table_indices.push(*wrapper_idx);
            } else {
                let import_idx = self
                    .import_ids
                    .get(&import_name)
                    .copied()
                    // Avoid panicking on malformed/partial import tables; route missing entries
                    // to the sentinel so lowering remains total and callers can surface errors.
                    .unwrap_or(sentinel_func_idx);
                func_to_index.insert(runtime_key, import_idx);
                table_indices.push(import_idx);
            }
        }

        let user_func_start = self.func_count;
        let user_func_count = ir.functions.len() as u32;
        let builtin_trampoline_count = builtin_trampoline_funcs.len() as u32;
        let builtin_trampoline_start = user_func_start + user_func_count;
        let user_trampoline_start = builtin_trampoline_start + builtin_trampoline_count;
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = (i as u32) + poll_table_prefix + builtin_table_len as u32;
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            func_to_index.insert(func_ir.name.clone(), user_func_start + i as u32);
            table_indices.push(user_func_start + i as u32);
        }
        let mut func_to_trampoline_idx = HashMap::new();
        for (i, (name, _)) in builtin_trampoline_funcs.iter().enumerate() {
            let idx = (i as u32)
                + poll_table_prefix
                + builtin_table_len as u32
                + ir.functions.len() as u32;
            func_to_trampoline_idx.insert(name.clone(), idx);
            table_indices.push(builtin_trampoline_start + i as u32);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = (i
                + poll_table_prefix as usize
                + builtin_table_len
                + ir.functions.len()
                + builtin_trampoline_funcs.len()) as u32;
            func_to_trampoline_idx.insert(func_ir.name.clone(), idx);
            table_indices.push(user_trampoline_start + i as u32);
        }

        let import_ids = self.import_ids.clone();
        let skipped_import_indices = self.skipped_import_indices.clone();
        let compile_ctx = CompileFuncContext {
            func_map: &func_to_table_idx,
            func_indices: &func_to_index,
            trampoline_map: &func_to_trampoline_idx,
            import_ids: &import_ids,
            reloc_enabled,
            table_base,
            multi_return_candidates: &multi_return_candidates,
            skipped_import_indices: &skipped_import_indices,
        };
        for func_ir in &ir.functions {
            let type_idx = if func_ir.name.ends_with("_poll") {
                2
            } else if let Some(&ret_count) = multi_return_candidates.get(&func_ir.name) {
                let key = (func_ir.params.len(), ret_count);
                *multi_return_type_map
                    .get(&key)
                    .unwrap_or(user_type_map.get(&func_ir.params.len()).unwrap_or(&0))
            } else {
                *user_type_map.get(&func_ir.params.len()).unwrap_or(&0)
            };
            self.compile_func(func_ir, type_idx, &compile_ctx);
        }

        if self.func_count != builtin_trampoline_start {
            panic!(
                "wasm builtin trampoline index mismatch: expected {builtin_trampoline_start}, got {}",
                self.func_count
            );
        }
        for (name, arity) in &builtin_trampoline_funcs {
            let target_idx = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline table slot missing for {name}"));
            let table_idx = table_base + table_slot;
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity: *arity,
                    has_closure: false,
                    kind: TrampolineKind::Plain,
                    closure_size: 0,
                },
                None,
            );
        }
        if self.func_count != user_trampoline_start {
            panic!(
                "wasm user trampoline index mismatch: expected {user_trampoline_start}, got {}",
                self.func_count
            );
        }
        for func_ir in &ir.functions {
            let (arity, has_closure) = *default_trampoline_spec
                .get(&func_ir.name)
                .unwrap_or_else(|| panic!("missing trampoline spec for {}", func_ir.name));
            let kind = task_kinds
                .get(&func_ir.name)
                .copied()
                .unwrap_or(TrampolineKind::Plain);
            let poll_name = if kind != TrampolineKind::Plain && !func_ir.name.ends_with("_poll") {
                format!("{}_poll", func_ir.name)
            } else {
                func_ir.name.clone()
            };
            let target_name = if kind != TrampolineKind::Plain {
                &poll_name
            } else {
                &func_ir.name
            };
            let target_idx = *func_to_index
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline target missing for {target_name}"));
            let table_slot = *func_to_table_idx
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline table slot missing for {target_name}"));
            let table_idx = table_base + table_slot;
            let closure_size = if kind == TrampolineKind::Plain {
                0
            } else {
                *task_closure_sizes
                    .get(&func_ir.name)
                    .unwrap_or_else(|| panic!("task closure size missing for {}", func_ir.name))
            };
            let mr_count = if kind == TrampolineKind::Plain {
                multi_return_candidates
                    .get(&func_ir.name)
                    .copied()
                    .filter(|&c| c > 1)
            } else {
                None
            };
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity,
                    has_closure,
                    kind,
                    closure_size,
                },
                mr_count,
            );
        }

        let mut element_section = None;
        let mut element_payload = None;
        if reloc_enabled {
            let table_init_index =
                self.compile_table_init(reloc_enabled, table_base, &table_indices);
            self.exports
                .export("molt_table_init", ExportKind::Func, table_init_index);
            let main_index = self
                .molt_main_index
                .unwrap_or_else(|| panic!("molt_main missing for table init wrapper"));
            let wrapper_index =
                self.compile_molt_main_wrapper(reloc_enabled, main_index, table_init_index);
            self.exports
                .export("molt_main", ExportKind::Func, wrapper_index);

            let mut ref_exported = HashSet::new();
            for func_index in &table_indices {
                if ref_exported.insert(*func_index) {
                    let name = format!("__molt_table_ref_{func_index}");
                    self.exports.export(&name, ExportKind::Func, *func_index);
                }
            }

            let mut payload = Vec::new();
            1u32.encode(&mut payload);
            payload.push(0x01);
            payload.push(0x00);
            (table_indices.len() as u32).encode(&mut payload);
            for func_index in &table_indices {
                encode_u32_leb128_padded(*func_index, &mut payload);
            }
            element_payload = Some(payload);
        } else {
            let mut section = ElementSection::new();
            let offset = ConstExpr::i32_const(table_base as i32);
            section.segment(ElementSegment {
                mode: ElementMode::Active {
                    table: None,
                    offset: &offset,
                },
                elements: Elements::Functions(Cow::Borrowed(&table_indices)),
            });
            element_section = Some(section);
        }

        let page_size: u64 = 64 * 1024;
        let required_pages = (self.data_offset as u64).div_ceil(page_size);
        let floor_pages = std::env::var("MOLT_WASM_MIN_PAGES")
            .ok()
            .and_then(|val| val.parse::<u64>().ok())
            .unwrap_or(64);
        let minimum_pages = required_pages.max(floor_pages);
        let memory_ty = MemoryType {
            minimum: minimum_pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        };
        self.imports
            .import("env", "memory", EntityType::Memory(memory_ty));
        self.exports.export("molt_memory", ExportKind::Memory, 0);

        // --- Import audit diagnostic (gated by MOLT_WASM_IMPORT_AUDIT=1) ---
        if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1") {
            let unused = self.import_ids.unused_names();
            let total = self.import_ids.len();
            let used = total - unused.len();
            let pct = if total > 0 {
                (unused.len() as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            eprintln!(
                "[molt-wasm-import-audit] {used}/{total} imports used, {} unused ({pct:.1}% bloat)",
                unused.len()
            );
            if !unused.is_empty() {
                eprintln!("[molt-wasm-import-audit] unused imports:");
                for name in &unused {
                    eprintln!("  - {name}");
                }
            }

            // --- Exception-related host call audit (Section 3.6) ---
            let eh_imports = [
                "exception_push",
                "exception_pop",
                "exception_pending",
                "exception_clear",
                "exception_new",
                "exception_new_from_class",
                "exception_kind",
                "exception_class",
                "exception_message",
                "exception_active",
                "exception_last",
                "exception_stack_clear",
                "exception_set_cause",
                "exception_set_value",
                "exception_context_set",
                "exception_set_last",
                "raise",
            ];
            let used_set = self.import_ids.used.borrow();
            let eh_used: Vec<&str> = eh_imports
                .iter()
                .copied()
                .filter(|name| used_set.contains(*name))
                .collect();
            let eh_eliminable: Vec<&str> = ["exception_push", "exception_pop", "exception_pending"]
                .iter()
                .copied()
                .filter(|name| used_set.contains(*name))
                .collect();
            drop(used_set);
            eprintln!(
                "[molt-wasm-import-audit] exception host calls: {}/{} used ({} eliminable by native EH: {})",
                eh_used.len(),
                eh_imports.len(),
                eh_eliminable.len(),
                eh_eliminable.join(", "),
            );
            if self.options.native_eh_enabled && !self.options.reloc_enabled {
                eprintln!("[molt-wasm-import-audit] native EH ENABLED: tag section emitted");
            } else if self.options.native_eh_enabled && self.options.reloc_enabled {
                eprintln!(
                    "[molt-wasm-import-audit] native EH requested but suppressed (reloc mode; wasm-ld doesn't support EH relocations)"
                );
            } else {
                eprintln!(
                    "[molt-wasm-import-audit] native EH disabled (set MOLT_WASM_NATIVE_EH=1)"
                );
            }

            // --- Tail call optimization audit (§3.5) ---
            eprintln!(
                "[molt-wasm-import-audit] tail calls emitted: {} (return_call instructions)",
                self.tail_calls_emitted
            );
        }

        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.funcs);
        self.module.section(&self.tables);
        self.module.section(&self.memories);

        // --- WASM EH Tag Section (Section 3.6) ---
        // Tag 0 = molt_exception with payload (i64) -> (), using type index 1.
        // Emitted between memory and export sections per WASM spec ordering.
        // Native EH requires non-relocatable output (wasm-ld doesn't support EH relocations)
        if self.options.native_eh_enabled && !self.options.reloc_enabled {
            let mut tags = TagSection::new();
            tags.tag(TagType {
                kind: TagKind::Exception,
                func_type_idx: TAG_EXCEPTION_FUNC_TYPE,
            });
            self.module.section(&tags);
        }

        self.module.section(&self.exports);
        if let Some(element_section) = element_section.as_ref() {
            self.module.section(element_section);
        }
        if let Some(payload) = element_payload.as_ref() {
            let raw_section = RawSection {
                id: 9,
                data: payload,
            };
            self.module.section(&raw_section);
        }
        self.module.section(&self.codes);
        self.module.section(&self.data);
        let mut bytes = self.module.finish();
        if reloc_enabled {
            bytes = add_reloc_sections(bytes, &self.data_segments, &self.data_relocs);
        }
        bytes
    }

    fn compile_trampoline(
        &mut self,
        reloc_enabled: bool,
        target_func_index: u32,
        table_idx: u32,
        spec: TrampolineSpec,
        multi_return_count: Option<usize>,
    ) {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
        } = spec;
        self.funcs.function(5);
        self.func_count += 1;
        let mut local_types = Vec::new();
        if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            local_types.push(ValType::I64);
            local_types.push(ValType::I32);
            local_types.push(ValType::I64);
            local_types.push(ValType::I32);
        }
        // For multi-value return trampolines (Plain kind only): allocate
        // N temp locals for the return values + 1 local for the tuple builder.
        // Params occupy locals 0..=2, so extra locals start at index 3.
        let mr_locals_start: u32 = 3 + local_types.len() as u32;
        if let (Some(ret_count), TrampolineKind::Plain) = (multi_return_count, &kind) {
            // N temp locals for storing each return value
            for _ in 0..ret_count {
                local_types.push(ValType::I64);
            }
            // 1 local for the tuple builder handle
            local_types.push(ValType::I64);
            let _ = ret_count; // suppress unused warning
        }
        let mut func = Function::new_with_locals_types(local_types);
        if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            let task_local = 3;
            let base_local = 4;
            let val_local = 5;
            let args_base_local = 6;
            match kind {
                TrampolineKind::Generator => {
                    if closure_size < 0 {
                        panic!("generator closure size must be non-negative");
                    }
                    let payload_slots = arity + usize::from(has_closure);
                    let needed = GEN_CONTROL_SIZE as i64 + (payload_slots as i64) * 8;
                    if closure_size < needed {
                        panic!("generator closure size too small for trampoline");
                    }
                    emit_table_index_i64(&mut func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(closure_size));
                    func.instruction(&Instruction::I64Const(TASK_KIND_GENERATOR));
                    emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
                    func.instruction(&Instruction::LocalSet(task_local));
                    // Zero-initialize the generator control block using
                    // bulk memory.fill instead of N i64.const 0 / i64.store
                    // sequences (WASM_OPTIMIZATION_PLAN Section 3.3).
                    func.instruction(&Instruction::LocalGet(task_local));
                    emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                    func.instruction(&Instruction::LocalSet(base_local));
                    func.instruction(&Instruction::LocalGet(base_local)); // dest
                    func.instruction(&Instruction::I32Const(0)); // fill value
                    func.instruction(&Instruction::I32Const(GEN_CONTROL_SIZE)); // byte count
                    func.instruction(&Instruction::MemoryFill(0));
                    if payload_slots > 0 {
                        if arity > 0 {
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalSet(args_base_local));
                        }
                        let mut offset = GEN_CONTROL_SIZE;
                        if has_closure {
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(0));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                            offset += 8;
                        }
                        for idx in 0..arity {
                            let arg_offset = offset + (idx as i32) * 8;
                            func.instruction(&Instruction::LocalGet(args_base_local));
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: (idx * std::mem::size_of::<u64>()) as u64,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(val_local));
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(arg_offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(val_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(val_local));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                        }
                    }
                    func.instruction(&Instruction::LocalGet(task_local));
                    func.instruction(&Instruction::End);
                    self.codes.function(&func);
                    return;
                }
                TrampolineKind::Coroutine => {
                    if closure_size < 0 {
                        panic!("coroutine closure size must be non-negative");
                    }
                    let payload_slots = arity + usize::from(has_closure);
                    let needed = (payload_slots as i64) * 8;
                    if closure_size < needed {
                        panic!("coroutine closure size too small for trampoline");
                    }
                    emit_table_index_i64(&mut func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(closure_size));
                    func.instruction(&Instruction::I64Const(TASK_KIND_COROUTINE));
                    emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
                    func.instruction(&Instruction::LocalSet(task_local));
                    if payload_slots > 0 {
                        func.instruction(&Instruction::LocalGet(task_local));
                        emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(base_local));
                        if arity > 0 {
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalSet(args_base_local));
                        }
                        let mut offset = 0;
                        if has_closure {
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(0));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                            offset += 8;
                        }
                        for idx in 0..arity {
                            let arg_offset = offset + (idx as i32) * 8;
                            func.instruction(&Instruction::LocalGet(args_base_local));
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: (idx * std::mem::size_of::<u64>()) as u64,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(val_local));
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(arg_offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(val_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(val_local));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                        }
                    }
                    func.instruction(&Instruction::LocalGet(task_local));
                    emit_call(
                        &mut func,
                        reloc_enabled,
                        self.import_ids["cancel_token_get_current"],
                    );
                    emit_call(
                        &mut func,
                        reloc_enabled,
                        self.import_ids["task_register_token_owned"],
                    );
                    func.instruction(&Instruction::Drop);
                    func.instruction(&Instruction::LocalGet(task_local));
                    func.instruction(&Instruction::End);
                    self.codes.function(&func);
                    return;
                }
                TrampolineKind::AsyncGen => {
                    if closure_size < 0 {
                        panic!("async generator closure size must be non-negative");
                    }
                    let payload_slots = arity + usize::from(has_closure);
                    let needed = GEN_CONTROL_SIZE as i64 + (payload_slots as i64) * 8;
                    if closure_size < needed {
                        panic!("async generator closure size too small for trampoline");
                    }
                    emit_table_index_i64(&mut func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(closure_size));
                    func.instruction(&Instruction::I64Const(TASK_KIND_GENERATOR));
                    emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
                    func.instruction(&Instruction::LocalSet(task_local));
                    // Zero-initialize the async generator control block
                    // using bulk memory.fill (WASM_OPTIMIZATION_PLAN
                    // Section 3.3).
                    func.instruction(&Instruction::LocalGet(task_local));
                    emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                    func.instruction(&Instruction::LocalSet(base_local));
                    func.instruction(&Instruction::LocalGet(base_local)); // dest
                    func.instruction(&Instruction::I32Const(0)); // fill value
                    func.instruction(&Instruction::I32Const(GEN_CONTROL_SIZE)); // byte count
                    func.instruction(&Instruction::MemoryFill(0));
                    if payload_slots > 0 {
                        if arity > 0 {
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalSet(args_base_local));
                        }
                        let mut offset = GEN_CONTROL_SIZE;
                        if has_closure {
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(0));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                            offset += 8;
                        }
                        for idx in 0..arity {
                            let arg_offset = offset + (idx as i32) * 8;
                            func.instruction(&Instruction::LocalGet(args_base_local));
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: (idx * std::mem::size_of::<u64>()) as u64,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(val_local));
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(arg_offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(val_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(val_local));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                        }
                    }
                    func.instruction(&Instruction::LocalGet(task_local));
                    emit_call(&mut func, reloc_enabled, self.import_ids["asyncgen_new"]);
                    func.instruction(&Instruction::End);
                    self.codes.function(&func);
                    return;
                }
                TrampolineKind::Plain => {}
            }
        }
        if has_closure {
            func.instruction(&Instruction::LocalGet(0));
        }
        for idx in 0..arity {
            func.instruction(&Instruction::LocalGet(1));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: (idx * std::mem::size_of::<u64>()) as u64,
                memory_index: 0,
            }));
        }
        emit_call(&mut func, reloc_enabled, target_func_index);
        if let Some(ret_count) = multi_return_count {
            // The target function pushed `ret_count` i64 values onto the
            // stack.  Pop them into temp locals (last return value is on
            // top, so store in reverse order) then reconstruct a tuple.
            let builder_local = mr_locals_start + ret_count as u32;
            for i in (0..ret_count).rev() {
                func.instruction(&Instruction::LocalSet(mr_locals_start + i as u32));
            }
            // list_builder_new(count) -> builder handle
            func.instruction(&Instruction::I64Const(box_int(ret_count as i64)));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["list_builder_new"],
            );
            func.instruction(&Instruction::LocalSet(builder_local));
            // list_builder_append(builder, value) for each value in order
            for i in 0..ret_count {
                func.instruction(&Instruction::LocalGet(builder_local));
                func.instruction(&Instruction::LocalGet(mr_locals_start + i as u32));
                emit_call(
                    &mut func,
                    reloc_enabled,
                    self.import_ids["list_builder_append"],
                );
            }
            // tuple_builder_finish(builder) -> tuple handle (single i64)
            func.instruction(&Instruction::LocalGet(builder_local));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["tuple_builder_finish"],
            );
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }

    fn compile_table_init(
        &mut self,
        reloc_enabled: bool,
        table_base: u32,
        table_indices: &[u32],
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(8);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        for (slot, target_index) in table_indices.iter().enumerate() {
            let table_index = table_base + slot as u32;
            emit_i32_const(&mut func, reloc_enabled, table_index as i32);
            emit_ref_func(&mut func, reloc_enabled, *target_index);
            func.instruction(&Instruction::TableSet(0));
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn compile_molt_main_wrapper(
        &mut self,
        reloc_enabled: bool,
        main_index: u32,
        table_init_index: u32,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(0);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        emit_call(&mut func, reloc_enabled, table_init_index);
        emit_call(&mut func, reloc_enabled, main_index);
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn compile_func(&mut self, func_ir: &FunctionIR, type_idx: u32, ctx: &CompileFuncContext<'_>) {
        let func_index = self.func_count;
        let reloc_enabled = ctx.reloc_enabled;
        self.funcs.function(type_idx);
        if reloc_enabled && func_ir.name == "molt_main" {
            self.molt_main_index = Some(func_index);
        } else {
            self.exports
                .export(&func_ir.name, ExportKind::Func, self.func_count);
        }
        self.func_count += 1;
        let func_map = ctx.func_map;
        let func_indices = ctx.func_indices;
        let trampoline_map = ctx.trampoline_map;
        let table_base = ctx.table_base;
        let import_ids = ctx.import_ids;
        let mut locals = HashMap::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if func_ir.name.ends_with("_poll") {
            let self_param_idx = locals.get("self").copied().unwrap_or(0);
            locals.insert("self_param".to_string(), self_param_idx);
            let self_idx = locals.get("self").copied();
            if self_idx.is_none() || self_idx == Some(self_param_idx) {
                locals.insert("self".to_string(), local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
            if local_count == 0 {
                local_count = 1;
            }
        }

        // --- Dead local elimination: pre-scan to find which IR variables are
        // ever *read* (appear in op.args or op.var).  Output-only variables
        // that are never read can share a single WASM local ("dead sink"),
        // reducing the total local count and binary size.
        let read_vars: HashSet<String> = {
            let mut s = HashSet::new();
            for op in &func_ir.ops {
                if let Some(args) = &op.args {
                    for arg in args {
                        s.insert(arg.clone());
                    }
                }
                if let Some(var) = &op.var {
                    s.insert(var.clone());
                }
            }
            s
        };
        // Also treat function parameters as always live.
        let param_set: HashSet<String> = func_ir.params.iter().cloned().collect();

        // --- Local variable coalescing (liveness analysis) ---
        // Compute live ranges for each variable: first write -> last read.
        // Variables whose ranges don't overlap can share a WASM local,
        // reducing total local count and binary size.
        let coalesced_map: HashMap<String, String> = {
            let mut first_write: HashMap<String, usize> = HashMap::new();
            let mut last_read: HashMap<String, usize> = HashMap::new();

            for (op_idx, op) in func_ir.ops.iter().enumerate() {
                if let Some(ref out) = op.out {
                    first_write.entry(out.clone()).or_insert(op_idx);
                }
                if let Some(ref args) = op.args {
                    for arg in args {
                        last_read.insert(arg.clone(), op_idx);
                    }
                }
                if let Some(ref var) = op.var {
                    last_read.insert(var.clone(), op_idx);
                }
            }

            // Build live ranges for coalescable temporaries only.
            // Only coalesce variables starting with __tmp or __v to be conservative.
            // Skip: parameters, dead-sink candidates (never read), _ptr/_len derivatives.
            let is_coalescable = |name: &str| -> bool {
                (name.starts_with("__tmp") || name.starts_with("__v"))
                    && !param_set.contains(name)
                    && read_vars.contains(name)
                    && !name.ends_with("_ptr")
                    && !name.ends_with("_len")
            };

            let mut ranges: Vec<(usize, usize, String)> = Vec::new();
            for (name, start) in &first_write {
                if !is_coalescable(name) {
                    continue;
                }
                let end = last_read.get(name).copied().unwrap_or(*start);
                ranges.push((*start, end, name.clone()));
            }

            // Sort by start position for greedy linear scan.
            ranges.sort_by_key(|r| r.0);

            // Greedy allocation: assign each variable to the lowest-numbered
            // "slot" (represented by the first variable that occupied it)
            // whose previous occupant's range has ended.
            // slot_end[i] = the end position of the variable currently in slot i.
            // slot_repr[i] = the representative variable name for slot i.
            let mut slot_end: Vec<usize> = Vec::new();
            let mut slot_repr: Vec<String> = Vec::new();
            let mut map: HashMap<String, String> = HashMap::new();

            for (start, end, name) in &ranges {
                // Find the lowest slot whose range has ended (end < start).
                let mut assigned = false;
                for (i, se) in slot_end.iter_mut().enumerate() {
                    if *se < *start {
                        // Reuse this slot: map this variable to the slot's representative.
                        *se = *end;
                        map.insert(name.clone(), slot_repr[i].clone());
                        assigned = true;
                        break;
                    }
                }
                if !assigned {
                    // Need a new slot; this variable is its own representative.
                    slot_end.push(*end);
                    slot_repr.push(name.clone());
                    map.insert(name.clone(), name.clone());
                }
            }

            map
        };

        // Allocate a single shared dead-sink local for output-only variables.
        let dead_sink_idx = local_count;
        locals.insert("__dead_sink".to_string(), dead_sink_idx);
        local_types.push(ValType::I64);
        local_count += 1;

        // ensure_local with dead-local awareness and coalescing: output-only
        // variables (never read) are mapped to the shared dead_sink_idx
        // instead of getting their own WASM local slot.  Coalescable
        // temporaries with non-overlapping lifetimes share locals via
        // the coalesced_map.  The `as_dead_out` flag indicates the caller
        // is allocating an output variable that should be checked against
        // the read set.
        let mut ensure_local_inner = |name: &str, as_dead_out: bool| -> u32 {
            if let Some(&idx) = locals.get(name) {
                return idx;
            }
            // Dead local elimination: if this is an output variable that
            // is never read and not a function parameter, reuse the
            // shared dead sink local.
            if as_dead_out && !read_vars.contains(name) && !param_set.contains(name) {
                locals.insert(name.to_string(), dead_sink_idx);
                return dead_sink_idx;
            }
            // Local coalescing: if this variable maps to a representative
            // that already has a local, reuse that local index.
            if let Some(repr) = coalesced_map.get(name) {
                if repr != name {
                    if let Some(&repr_idx) = locals.get(repr) {
                        locals.insert(name.to_string(), repr_idx);
                        return repr_idx;
                    }
                }
            }
            let idx = local_count;
            locals.insert(name.to_string(), idx);
            local_types.push(ValType::I64);
            local_count += 1;
            idx
        };

        let mut needs_field_fast = false;
        let mut needs_alloc_resolve = false;
        let mut stateful = false;
        let mut saw_jump_or_label = false;
        let mut fast_int_count: usize = 0;
        let mut const_seed_seen: HashSet<String> = HashSet::new();
        let mut const_seed_locals_all: Vec<(u32, i64)> = Vec::new();
        let mut defined_vars: HashSet<String> = HashSet::new();
        let mut used_vars: HashSet<String> = HashSet::new();
        for op in &func_ir.ops {
            if let Some(args) = &op.args {
                for arg in args {
                    if arg != "self" && arg != "none" && arg.starts_with('v') {
                        used_vars.insert(arg.clone());
                    }
                }
            }
            if let Some(out) = &op.out {
                if out != "none" {
                    defined_vars.insert(out.clone());
                }
            }
        }
        for op in &func_ir.ops {
            if op.fast_int.unwrap_or(false) {
                fast_int_count += 1;
            }
            if let Some(args) = &op.args {
                for arg in args {
                    ensure_local_inner(arg, false);
                }
            }
            if let Some(out) = &op.out {
                let out_local_idx = ensure_local_inner(out, true);
                let is_dead = out_local_idx == dead_sink_idx;
                if op.kind == "const_str" || op.kind == "const_bytes" || op.kind == "const_bigint" {
                    // _ptr and _len locals are used internally by the op
                    // emission so they always need real (non-sink) locals.
                    ensure_local_inner(&format!("{out}_ptr"), false);
                    ensure_local_inner(&format!("{out}_len"), false);
                }
                if !const_seed_seen.contains(out) {
                    let bits = match op.kind.as_str() {
                        "const" => op.value.map(box_int),
                        "const_bool" => op.value.map(box_bool),
                        "const_float" => op.f_value.map(box_float),
                        "const_none" => Some(box_none()),
                        _ => None,
                    };
                    if let Some(bits) = bits {
                        // Skip seeding dead locals -- the value is never
                        // observed so there is no point initializing it.
                        if !is_dead {
                            const_seed_seen.insert(out.clone());
                            const_seed_locals_all.push((out_local_idx, bits));
                        }
                    }
                }
            }
            match op.kind.as_str() {
                "store" | "store_init" | "load" | "guarded_load" | "guarded_field_get"
                | "guarded_field_set" | "guarded_field_init" => needs_field_fast = true,
                "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                | "chan_recv_yield" => stateful = true,
                "jump" | "label" => saw_jump_or_label = true,
                "alloc_task" => {
                    let tk = op.task_kind.as_deref().unwrap_or("future");
                    let has_prefix = tk == "generator";
                    let has_args = op.args.as_ref().map_or(false, |a| !a.is_empty());
                    if has_prefix || has_args {
                        needs_alloc_resolve = true;
                    }
                }
                _ => {}
            }
        }

        // Safety: seed undefined variables (used but never defined) with
        // box_none().  This can happen when front-end IR omits a const_none
        // definition due to module-context differences (e.g. genexpr compiled
        // for import vs __main__).  Without this, the WASM local defaults to
        // 0 which is not a valid boxed value and causes runtime crashes.
        for undef in used_vars.difference(&defined_vars) {
            if let Some(&local_idx) = locals.get(undef.as_str()) {
                if local_idx != dead_sink_idx && !const_seed_seen.contains(undef) {
                    const_seed_seen.insert(undef.clone());
                    const_seed_locals_all.push((local_idx, box_none()));
                }
            }
        }

        if needs_field_fast {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                locals.entry("__wasm_tmp0".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I32);
                local_count += 1;
            }
            if let std::collections::hash_map::Entry::Vacant(entry) =
                locals.entry("__wasm_tmp1".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
        }

        if needs_alloc_resolve {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                locals.entry("__wasm_alloc_resolve".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I32);
                local_count += 1;
            }
        }

        for name in ["__molt_tmp0", "__molt_tmp1", "__molt_tmp2", "__molt_tmp3"] {
            if let std::collections::hash_map::Entry::Vacant(entry) = locals.entry(name.to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
        }

        // Constant materialization cache: when a function body has 3+ fast_int
        // ops, pre-allocate WASM locals for the constants that would otherwise
        // be emitted as i64.const immediates dozens of times (INT_SHIFT,
        // INT_MIN_INLINE, INT_MAX_INLINE).  Below the threshold the overhead
        // of initializing the locals exceeds the savings.
        let const_cache = if fast_int_count >= 3 {
            let shift_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let min_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let max_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            ConstantCache {
                int_shift: Some(shift_idx),
                int_min: Some(min_idx),
                int_max: Some(max_idx),
            }
        } else {
            ConstantCache::default()
        };

        let jumpful = !stateful && saw_jump_or_label;

        // --- Tail call optimization eligibility (WASM tail calls proposal §3.5) ---
        // A function is eligible for tail call optimization when it has no
        // exception handling (exception_push/pop), which would require cleanup
        // between the call and return.  Only non-stateful functions are
        // candidates since stateful dispatch emits ops one-at-a-time.
        let has_exception_handling = func_ir
            .ops
            .iter()
            .any(|op| op.kind == "exception_push" || op.kind == "exception_pop");
        let tail_call_eligible = !stateful && !has_exception_handling;

        if stateful && !locals.contains_key("self_param") {
            let self_param_idx = locals
                .get("self")
                .copied()
                .or_else(|| {
                    func_ir
                        .params
                        .first()
                        .and_then(|name| locals.get(name))
                        .copied()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "stateful wasm function {} missing self parameter",
                        func_ir.name
                    )
                });
            locals.insert("self_param".to_string(), self_param_idx);
            locals.entry("self".to_string()).or_insert(self_param_idx);
        }
        let self_ptr_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let block_map_base_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let return_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_remap_base_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_remap_value_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let const_seed_locals = if stateful || jumpful {
            const_seed_locals_all
        } else {
            Vec::new()
        };

        // --- Multi-value return optimization locals (Section 3.1) ---
        let multi_return_candidates = ctx.multi_return_candidates;
        let is_multi_return_callee = multi_return_candidates.get(&func_ir.name).copied();

        let mut multi_ret_locals: Vec<u32> = Vec::new();
        let mut multi_ret_tuple_vars: HashSet<String> = HashSet::new();
        if let Some(ret_count) = is_multi_return_callee {
            for i in 0..ret_count {
                let name = format!("__multi_ret_{i}");
                if !locals.contains_key(&name) {
                    locals.insert(name, local_count);
                    local_types.push(ValType::I64);
                    multi_ret_locals.push(local_count);
                    local_count += 1;
                }
            }
            for op in &func_ir.ops {
                if op.kind == "tuple_new" {
                    if let Some(args) = &op.args {
                        if args.len() == ret_count {
                            if let Some(out) = &op.out {
                                multi_ret_tuple_vars.insert(out.clone());
                            }
                        }
                    }
                }
            }
        }

        let mut multi_ret_call_locals: HashMap<(String, i64), u32> = HashMap::new();
        let mut multi_ret_call_vars: HashSet<String> = HashSet::new();
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if op.kind != "call_internal" {
                continue;
            }
            let Some(callee) = op.s_value.as_ref() else {
                continue;
            };
            let Some(&ret_count) = multi_return_candidates.get(callee) else {
                continue;
            };
            let Some(result_var) = op.out.as_ref() else {
                continue;
            };
            let mut valid = true;
            for k in 0..ret_count {
                let j = op_idx + 1 + k;
                if j >= func_ir.ops.len() {
                    valid = false;
                    break;
                }
                let next_op = &func_ir.ops[j];
                if next_op.kind != "tuple_index" {
                    valid = false;
                    break;
                }
                let Some(args) = next_op.args.as_ref() else {
                    valid = false;
                    break;
                };
                if args.len() < 2 || args[0] != *result_var {
                    valid = false;
                    break;
                }
            }
            if !valid {
                continue;
            }
            multi_ret_call_vars.insert(result_var.clone());
            for k in 0..ret_count {
                let name = format!("__multi_call_{result_var}_{k}");
                if !locals.contains_key(&name) {
                    locals.insert(name.clone(), local_count);
                    local_types.push(ValType::I64);
                    local_count += 1;
                }
                multi_ret_call_locals.insert((result_var.clone(), k as i64), locals[&name]);
            }
        }

        let _ = local_count;
        let mut func = Function::new_with_locals_types(local_types);
        #[derive(Clone, Copy)]
        enum ControlKind {
            Block,
            Loop,
            If,
            Try,
        }
        let mut control_stack: Vec<ControlKind> = Vec::new();
        let mut try_stack: Vec<usize> = Vec::new();
        let mut label_stack: Vec<i64> = Vec::new();
        let mut label_depths: HashMap<i64, usize> = HashMap::new();

        let dispatch_blocks = if stateful || jumpful {
            let (block_starts, block_for_op) = build_dispatch_blocks(&func_ir.ops);
            let block_map_bytes = build_dispatch_block_map(&block_for_op);
            let block_map_segment = self.add_data_segment(reloc_enabled, &block_map_bytes);
            Some((block_starts, block_map_segment))
        } else {
            None
        };
        let dispatch_control_maps = if stateful || jumpful {
            Some(build_dispatch_control_maps(&func_ir.ops, stateful))
        } else {
            None
        };
        let state_resume_maps = if stateful {
            let (state_map, const_ints) = build_state_resume_maps(&func_ir.ops);
            let state_remap_table = build_dense_state_remap_table(&state_map).map(|remap_bytes| {
                let remap_entries = (remap_bytes.len() / std::mem::size_of::<i64>()) as i64;
                let remap_segment = self.add_data_segment(reloc_enabled, &remap_bytes);
                (remap_entries, remap_segment)
            });
            Some((state_map, const_ints, state_remap_table))
        } else {
            None
        };
        if let Some((_, block_map_segment)) = dispatch_blocks.as_ref() {
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for dispatch");
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, *block_map_segment);
            func.instruction(&Instruction::LocalSet(block_map_base_local));
        }
        if let Some((_, _, Some((_, remap_segment)))) = state_resume_maps.as_ref() {
            let remap_base_local =
                state_remap_base_local.expect("state remap base local missing for stateful wasm");
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, *remap_segment);
            func.instruction(&Instruction::LocalSet(remap_base_local));
        }
        if stateful || jumpful {
            // Seed dispatch locals from their first literal assignment so control-flow
            // edge threading cannot observe a raw wasm zero (0.0 bits) for an
            // otherwise integer/none local before its defining block executes.
            for (local_idx, bits) in const_seed_locals.iter().copied() {
                func.instruction(&Instruction::I64Const(bits));
                func.instruction(&Instruction::LocalSet(local_idx));
            }
        }

        // Initialize constant materialization cache (once per function entry).
        const_cache.emit_init(&mut func);

        // Capture native_eh_enabled before the closure to avoid borrowing self.
        // Native EH requires non-relocatable output (wasm-ld doesn't support EH relocations)
        let native_eh_enabled = self.options.native_eh_enabled && !self.options.reloc_enabled;

        // Tail call optimization counter (WASM tail calls proposal §3.5).
        // Uses Cell so the closure can mutate it while also being borrowed
        // by multiple call sites (stateful dispatch emits ops one-at-a-time).
        let tail_call_count: Cell<usize> = Cell::new(0);

        let mut emit_ops = |func: &mut Function,
                            ops: &[OpIR],
                            control_stack: &mut Vec<ControlKind>,
                            try_stack: &mut Vec<usize>,
                            label_stack: &mut Vec<i64>,
                            label_depths: &mut HashMap<i64, usize>,
                            base_idx: usize| {
            // Peephole state: track WASM locals whose raw (unboxed) integer
            // value is known at compile time.  Populated by `const` ops;
            // invalidated when a local is overwritten by a non-const op or
            // control flow diverges.
            let mut known_raw_ints: HashMap<u32, i64> = HashMap::new();

            // Tail call skip flag: when we emit a return_call for a
            // call_internal op, we set this to skip the immediately
            // following `ret` op that is now subsumed.
            let mut skip_next = false;

            for (rel_idx, op) in ops.iter().enumerate() {
                let op_idx = base_idx + rel_idx;

                if skip_next {
                    skip_next = false;
                    continue;
                }

                match op.kind.as_str() {
                    "const" => {
                        let val = op.value.unwrap();
                        func.instruction(&Instruction::I64Const(box_int(val)));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                        // Record the known raw value for this local so
                        // subsequent fast_int unbox can be elided.
                        known_raw_ints.insert(local_idx, val);
                    }
                    "const_bool" => {
                        let val = op.value.unwrap();
                        func.instruction(&Instruction::I64Const(box_bool(val)));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_float" => {
                        let val = op.f_value.expect("Float value not found");
                        func.instruction(&Instruction::I64Const(box_float(val)));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_none" => {
                        func.instruction(&Instruction::I64Const(box_none()));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_not_implemented" => {
                        emit_call(func, reloc_enabled, import_ids["not_implemented"]);
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_ellipsis" => {
                        emit_call(func, reloc_enabled, import_ids["ellipsis"]);
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_str" => {
                        let out_name = op.out.as_ref().unwrap();
                        let bytes = op
                            .bytes
                            .as_deref()
                            .unwrap_or_else(|| op.s_value.as_ref().unwrap().as_bytes());
                        let data = self.add_data_segment(reloc_enabled, bytes);

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::I64Const(8));
                        emit_call(func, reloc_enabled, import_ids["alloc"]);
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["string_from_bytes"]);
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "const_bigint" => {
                        let s = op.s_value.as_ref().unwrap();
                        let out_name = op.out.as_ref().unwrap();
                        let bytes = s.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        emit_call(func, reloc_enabled, import_ids["bigint_from_str"]);
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "const_bytes" => {
                        let bytes = op.bytes.as_ref().expect("Bytes not found");
                        let out_name = op.out.as_ref().unwrap();
                        let data = self.add_data_segment(reloc_enabled, bytes);

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::I64Const(8));
                        emit_call(func, reloc_enabled, import_ids["alloc"]);
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["bytes_from_bytes"]);
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "add" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Add);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["add"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Add);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["add"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_add" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Add);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_add"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Add);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_add"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_range_iter" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_range_iter"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_range_iter_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["vec_sum_int_range_iter_trusted"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_range_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_float" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_float"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_float_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_float_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_float_range_iter" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_float_range_iter"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_float_range_iter_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["vec_sum_float_range_iter_trusted"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_float_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_float_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_float_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["vec_sum_float_range_trusted"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_prod_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_prod_int_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_prod_int_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["vec_prod_int_range_trusted"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_min_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_min_int_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_min_int_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_min_int_range_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_max_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_max_int_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_max_int_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        emit_call(func, reloc_enabled, import_ids["vec_max_int_range_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "sub" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["sub"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Sub);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["sub"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "mul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Mul);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["mul"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Mul);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["mul"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_sub" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_sub"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Sub);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_sub"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_mul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Mul);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_mul"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Mul);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_mul"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Or);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["bit_or"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["bit_or"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["bit_and"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["bit_and"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_xor" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Xor);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["bit_xor"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["bit_xor"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "invert" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["invert"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_bit_or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Or);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_bit_or"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_bit_or"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_bit_and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_bit_and"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_bit_and"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_bit_xor" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Xor);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_bit_xor"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_bit_xor"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "lshift" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64GeS);
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64Const(64));
                            func.instruction(&Instruction::I64LtS);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64Shl);
                            func.instruction(&Instruction::LocalSet(tmp_raw));

                            func.instruction(&Instruction::LocalGet(tmp_raw));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64ShrS);
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::I64Eq);
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["lshift"]);
                            func.instruction(&Instruction::End);

                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["lshift"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["lshift"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "rshift" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64GeS);
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64Const(64));
                            func.instruction(&Instruction::I64LtS);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64ShrS);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["rshift"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["rshift"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["matmul"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "div" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            emit_unbox_int_local_trusted_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::F64ConvertI64S);
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::F64ConvertI64S);
                            func.instruction(&Instruction::F64Div);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["div"]);
                            func.instruction(&Instruction::End);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Div);
                            func.instruction(&Instruction::I64ReinterpretF64);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["div"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "floordiv" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64DivS);
                            func.instruction(&Instruction::LocalSet(tmp_raw));

                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64RemS);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64LtS);
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64LtS);
                            func.instruction(&Instruction::I32Xor);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(tmp_raw));
                            func.instruction(&Instruction::I64Const(1));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            func.instruction(&Instruction::End);

                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["floordiv"]);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["floordiv"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["floordiv"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "mod" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            let tmp_raw = locals["__molt_tmp2"];
                            emit_unbox_int_local_trusted_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64RemS);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            func.instruction(&Instruction::LocalGet(tmp_raw));
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::LocalGet(tmp_lhs));
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64LtS);
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64LtS);
                            func.instruction(&Instruction::I32Xor);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(tmp_raw));
                            func.instruction(&Instruction::LocalGet(tmp_rhs));
                            func.instruction(&Instruction::I64Add);
                            func.instruction(&Instruction::LocalSet(tmp_raw));
                            func.instruction(&Instruction::End);
                            emit_inline_int_range_check(func, tmp_raw, &const_cache);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["mod"]);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["mod"]);
                            func.instruction(&Instruction::End);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["mod"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "pow" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["pow"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "pow_mod" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        let modulus = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::LocalGet(modulus));
                        emit_call(func, reloc_enabled, import_ids["pow_mod"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "round" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let ndigits = locals[&args[1]];
                        let has_ndigits = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(ndigits));
                        func.instruction(&Instruction::LocalGet(has_ndigits));
                        emit_call(func, reloc_enabled, import_ids["round"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "trunc" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["trunc"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "lt" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64LtS);
                            emit_box_bool_from_i32(func);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Lt);
                            emit_box_bool_from_i32(func);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["lt"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "le" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64LeS);
                            emit_box_bool_from_i32(func);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Le);
                            emit_box_bool_from_i32(func);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["le"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gt" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64GtS);
                            emit_box_bool_from_i32(func);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Gt);
                            emit_box_bool_from_i32(func);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["gt"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ge" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            let tmp_lhs = locals["__molt_tmp0"];
                            let tmp_rhs = locals["__molt_tmp1"];
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                lhs,
                                tmp_lhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            emit_unbox_int_local_trusted_tee_opt(
                                func,
                                rhs,
                                tmp_rhs,
                                &const_cache,
                                &known_raw_ints,
                            );
                            func.instruction(&Instruction::I64GeS);
                            emit_box_bool_from_i32(func);
                        } else {
                            // fast_float: check if both operands are plain f64
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Const(48));
                            func.instruction(&Instruction::I64ShrU);
                            func.instruction(&Instruction::I64Const(0x7FF9));
                            func.instruction(&Instruction::I64Sub);
                            func.instruction(&Instruction::I64Const(5));
                            func.instruction(&Instruction::I64LtU);
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::I32And);
                            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::F64ReinterpretI64);
                            func.instruction(&Instruction::F64Ge);
                            emit_box_bool_from_i32(func);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["ge"]);
                            func.instruction(&Instruction::End);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "eq" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            // Box/unbox elimination: when both operands are
                            // known NaN-boxed integers, equality of the boxed
                            // representations implies equality of the raw
                            // values (same tag prefix).  Skip unbox entirely.
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Eq);
                            emit_box_bool_from_i32(func);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["eq"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ne" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.fast_int.unwrap_or(false) {
                            // Box/unbox elimination: compare NaN-boxed values
                            // directly — same tag means ne(boxed) iff ne(raw).
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            func.instruction(&Instruction::I64Ne);
                            emit_box_bool_from_i32(func);
                        } else {
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["ne"]);
                        }
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_eq" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["string_eq"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["is"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "not" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["not"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "abs" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["abs_builtin"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::End);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::End);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "contains" => {
                        let args = op.args.as_ref().unwrap();
                        let container = locals[&args[0]];
                        let item = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(container));
                        func.instruction(&Instruction::LocalGet(item));
                        emit_call(func, reloc_enabled, import_ids["contains"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "guard_type" | "guard_tag" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let expected = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_type"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "guard_layout" | "guard_dict_shape" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "print" => {
                        let args = op.args.as_ref().unwrap();
                        if let Some(&idx) = locals.get(&args[0]) {
                            func.instruction(&Instruction::LocalGet(idx));
                            emit_call(func, reloc_enabled, import_ids["print_obj"]);
                        }
                    }
                    "print_newline" => {
                        emit_call(func, reloc_enabled, import_ids["print_newline"]);
                    }
                    "alloc" => {
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        emit_call(func, reloc_enabled, import_ids["alloc"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "alloc_class" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "alloc_class_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class_trusted"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "alloc_class_static" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class_static"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "json_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                            let ptr = locals
                                .get(&format!("{arg_name}_ptr"))
                                .copied()
                                .unwrap_or(locals[arg_name]);
                            let tmp_rc = locals["__molt_tmp0"];

                            func.instruction(&Instruction::I64Const(8));
                            emit_call(func, reloc_enabled, import_ids["alloc"]);
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out_ptr));

                            func.instruction(&Instruction::LocalGet(ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(len));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["json_parse_scalar"]);
                            func.instruction(&Instruction::I64ExtendI32U);
                            func.instruction(&Instruction::LocalSet(tmp_rc));

                            func.instruction(&Instruction::LocalGet(tmp_rc));
                            func.instruction(&Instruction::I64Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["json_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::End);
                        } else {
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["json_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                        }
                    }
                    "msgpack_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                            let ptr = locals
                                .get(&format!("{arg_name}_ptr"))
                                .copied()
                                .unwrap_or(locals[arg_name]);
                            let tmp_rc = locals["__molt_tmp0"];

                            func.instruction(&Instruction::I64Const(8));
                            emit_call(func, reloc_enabled, import_ids["alloc"]);
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out_ptr));

                            func.instruction(&Instruction::LocalGet(ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(len));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar"]);
                            func.instruction(&Instruction::I64ExtendI32U);
                            func.instruction(&Instruction::LocalSet(tmp_rc));

                            func.instruction(&Instruction::LocalGet(tmp_rc));
                            func.instruction(&Instruction::I64Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::End);
                        } else {
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                        }
                    }
                    "cbor_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                            let ptr = locals
                                .get(&format!("{arg_name}_ptr"))
                                .copied()
                                .unwrap_or(locals[arg_name]);
                            let tmp_rc = locals["__molt_tmp0"];

                            func.instruction(&Instruction::I64Const(8));
                            emit_call(func, reloc_enabled, import_ids["alloc"]);
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out_ptr));

                            func.instruction(&Instruction::LocalGet(ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(len));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar"]);
                            func.instruction(&Instruction::I64ExtendI32U);
                            func.instruction(&Instruction::LocalSet(tmp_rc));

                            func.instruction(&Instruction::LocalGet(tmp_rc));
                            func.instruction(&Instruction::I64Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::End);
                        } else {
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                        }
                    }
                    "len" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["len"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "id" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["id"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ord" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["ord"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "chr" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["chr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_new" => {
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "list_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(box_int(args.len() as i64)));
                        emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        emit_call(func, reloc_enabled, import_ids["list_builder_finish"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "range_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        let start = locals[&args[0]];
                        let stop = locals[&args[1]];
                        let step = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        func.instruction(&Instruction::LocalGet(step));
                        emit_call(func, reloc_enabled, import_ids["range_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "list_from_range" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        let start = locals[&args[0]];
                        let stop = locals[&args[1]];
                        let step = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        func.instruction(&Instruction::LocalGet(step));
                        emit_call(func, reloc_enabled, import_ids["list_from_range"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "tuple_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out_name = op.out.as_ref().unwrap();
                        let out = locals[out_name];
                        // Multi-value return (Section 3.1): store elements
                        // into __multi_ret_N locals instead of heap-allocating
                        // when this tuple flows directly to a return in a
                        // candidate function.
                        if is_multi_return_callee.is_some()
                            && multi_ret_tuple_vars.contains(out_name)
                            && args.len() == multi_ret_locals.len()
                        {
                            for (k, arg_name) in args.iter().enumerate() {
                                let val = locals[arg_name];
                                func.instruction(&Instruction::LocalGet(val));
                                func.instruction(&Instruction::LocalSet(multi_ret_locals[k]));
                            }
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::LocalSet(out));
                        } else {
                            func.instruction(&Instruction::I64Const(box_int(args.len() as i64)));
                            emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
                            func.instruction(&Instruction::LocalSet(out));
                            for name in args {
                                let val = locals[name];
                                func.instruction(&Instruction::LocalGet(out));
                                func.instruction(&Instruction::LocalGet(val));
                                emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
                            }
                            func.instruction(&Instruction::LocalGet(out));
                            emit_call(func, reloc_enabled, import_ids["tuple_builder_finish"]);
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "callargs_push_pos" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_push_kw" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let name = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_kw"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_expand_star" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let iterable = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(iterable));
                        emit_call(func, reloc_enabled, import_ids["callargs_expand_star"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_expand_kwstar" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let mapping = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(mapping));
                        emit_call(func, reloc_enabled, import_ids["callargs_expand_kwstar"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_append" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_append"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["list_pop"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_extend" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["list_extend"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_insert" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_insert"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_remove"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_clear"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_copy" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_copy"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_reverse" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_reverse"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_count" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_index" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_index"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_index_range" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        let start = locals[&args[2]];
                        let stop = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        emit_call(func, reloc_enabled, import_ids["list_index_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_from_list" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["tuple_from_list"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const((args.len() / 2) as i64));
                        emit_call(func, reloc_enabled, import_ids["dict_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for pair in args.chunks(2) {
                            let key = locals[&pair[0]];
                            let val = locals[&pair[1]];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(key));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["dict_set"]);
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "dict_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["dict_from_obj"]);
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "set_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["set_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["set_add"]);
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "frozenset_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["frozenset_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_get" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let default = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["dict_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_inc" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let delta = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(delta));
                        emit_call(func, reloc_enabled, import_ids["dict_inc"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_str_int_inc" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let delta = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(delta));
                        emit_call(func, reloc_enabled, import_ids["dict_str_int_inc"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split_ws_dict_inc" => {
                        let args = op.args.as_ref().unwrap();
                        let line = locals[&args[0]];
                        let dict = locals[&args[1]];
                        let delta = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(line));
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(delta));
                        emit_call(func, reloc_enabled, import_ids["string_split_ws_dict_inc"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "taq_ingest_line" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let line = locals[&args[1]];
                        let bucket_size = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(line));
                        func.instruction(&Instruction::LocalGet(bucket_size));
                        emit_call(func, reloc_enabled, import_ids["taq_ingest_line"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split_sep_dict_inc" => {
                        let args = op.args.as_ref().unwrap();
                        let line = locals[&args[0]];
                        let sep = locals[&args[1]];
                        let dict = locals[&args[2]];
                        let delta = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(line));
                        func.instruction(&Instruction::LocalGet(sep));
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(delta));
                        emit_call(func, reloc_enabled, import_ids["string_split_sep_dict_inc"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let default = locals[&args[2]];
                        let has_default = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        func.instruction(&Instruction::LocalGet(has_default));
                        emit_call(func, reloc_enabled, import_ids["dict_pop"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_setdefault" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let default = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["dict_setdefault"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_setdefault_empty_list" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["dict_setdefault_empty_list"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_update" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["dict_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_clear"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_copy" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_copy"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_popitem" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_popitem"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_update_kwstar" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["dict_update_kwstar"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_add"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "frozenset_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_discard" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_discard"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_remove"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        emit_call(func, reloc_enabled, import_ids["set_pop"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_intersection_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_intersection_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_difference_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_difference_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_symdiff_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_symdiff_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_keys" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_keys"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_values" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_values"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_items" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_items"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_count" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(tuple));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["tuple_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_index" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple_var = &args[0];
                        let res = locals[op.out.as_ref().unwrap()];
                        // Multi-value return (Section 3.1): if the tuple was
                        // produced by a promoted call_internal, the values
                        // are already in dedicated locals.
                        if multi_ret_call_vars.contains(tuple_var) {
                            let idx = op.value.unwrap_or(0);
                            if let Some(&src_local) =
                                multi_ret_call_locals.get(&(tuple_var.clone(), idx))
                            {
                                func.instruction(&Instruction::LocalGet(src_local));
                                func.instruction(&Instruction::LocalSet(res));
                            } else {
                                let tuple = locals[tuple_var];
                                let val = locals[&args[1]];
                                func.instruction(&Instruction::LocalGet(tuple));
                                func.instruction(&Instruction::LocalGet(val));
                                emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                                func.instruction(&Instruction::LocalSet(res));
                            }
                        } else {
                            let tuple = locals[tuple_var];
                            let val = locals[&args[1]];
                            func.instruction(&Instruction::LocalGet(tuple));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                            func.instruction(&Instruction::LocalSet(res));
                        }
                    }
                    "iter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["iter"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "enumerate" => {
                        let args = op.args.as_ref().unwrap();
                        let iterable = locals[&args[0]];
                        let start = locals[&args[1]];
                        let has_start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(iterable));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(has_start));
                        emit_call(func, reloc_enabled, import_ids["enumerate"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "aiter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["aiter"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "iter_next" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["iter_next"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "anext" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["anext"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "asyncgen_new" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        emit_call(func, reloc_enabled, import_ids["asyncgen_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "asyncgen_shutdown" => {
                        emit_call(func, reloc_enabled, import_ids["asyncgen_shutdown"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_send" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["generator_send"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_throw" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["generator_throw"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_close" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        emit_call(func, reloc_enabled, import_ids["generator_close"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_generator" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_generator"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_bound_method" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_bound_method"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_callable" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_callable"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["index"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "store_index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["store_index"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "del_index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["del_index"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "slice" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let start = locals[&args[1]];
                        let end = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        emit_call(func, reloc_enabled, import_ids["slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "slice_new" => {
                        let args = op.args.as_ref().unwrap();
                        let start = locals[&args[0]];
                        let stop = locals[&args[1]];
                        let step = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        func.instruction(&Instruction::LocalGet(step));
                        emit_call(func, reloc_enabled, import_ids["slice_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_find"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_find_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytes_find_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_find"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_find_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytearray_find_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_find"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_find_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["string_find_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_format" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let spec = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(spec));
                        emit_call(func, reloc_enabled, import_ids["format_builtin"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_startswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_startswith_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["string_startswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_startswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_startswith_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytes_startswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_startswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_startswith_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["bytearray_startswith_slice"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_endswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_endswith_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["string_endswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_endswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_endswith_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytes_endswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_endswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_endswith_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytearray_endswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_count_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["string_count_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_count_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytes_count_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_count_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["bytearray_count_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "env_get" => {
                        let args = op.args.as_ref().unwrap();
                        let key = locals[&args[0]];
                        let default = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["env_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "errno_constants" => {
                        emit_call(func, reloc_enabled, import_ids["errno_constants"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_join" => {
                        let args = op.args.as_ref().unwrap();
                        let sep = locals[&args[0]];
                        let items = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sep));
                        func.instruction(&Instruction::LocalGet(items));
                        emit_call(func, reloc_enabled, import_ids["string_join"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_split"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split_max" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let maxsplit = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(maxsplit));
                        emit_call(func, reloc_enabled, import_ids["string_split_max"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "statistics_mean_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let start = locals[&args[1]];
                        let end = locals[&args[2]];
                        let has_start = locals[&args[3]];
                        let has_end = locals[&args[4]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["statistics_mean_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "statistics_stdev_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let start = locals[&args[1]];
                        let end = locals[&args[2]];
                        let has_start = locals[&args[3]];
                        let has_end = locals[&args[4]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        emit_call(func, reloc_enabled, import_ids["statistics_stdev_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_lower" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_lower"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_upper" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_upper"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_capitalize" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_capitalize"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_strip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_strip"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_lstrip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_lstrip"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_rstrip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_rstrip"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_split"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_split_max" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let maxsplit = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(maxsplit));
                        emit_call(func, reloc_enabled, import_ids["bytes_split_max"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_split"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_split_max" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let maxsplit = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(maxsplit));
                        emit_call(func, reloc_enabled, import_ids["bytearray_split_max"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        let count = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::LocalGet(count));
                        emit_call(func, reloc_enabled, import_ids["bytes_replace"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        let count = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::LocalGet(count));
                        emit_call(func, reloc_enabled, import_ids["string_replace"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        let count = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::LocalGet(count));
                        emit_call(func, reloc_enabled, import_ids["bytearray_replace"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["bytes_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_from_str" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        let encoding = locals[&args[1]];
                        let errors = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalGet(encoding));
                        func.instruction(&Instruction::LocalGet(errors));
                        emit_call(func, reloc_enabled, import_ids["bytes_from_str"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["bytearray_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_from_str" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        let encoding = locals[&args[1]];
                        let errors = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalGet(encoding));
                        func.instruction(&Instruction::LocalGet(errors));
                        emit_call(func, reloc_enabled, import_ids["bytearray_from_str"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "float_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["float_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "int_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let base = locals[&args[1]];
                        let has_base = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(base));
                        func.instruction(&Instruction::LocalGet(has_base));
                        emit_call(func, reloc_enabled, import_ids["int_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "complex_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let imag = locals[&args[1]];
                        let has_imag = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(imag));
                        func.instruction(&Instruction::LocalGet(has_imag));
                        emit_call(func, reloc_enabled, import_ids["complex_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "intarray_from_seq" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["intarray_from_seq"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_new" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["memoryview_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_tobytes" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["memoryview_tobytes"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_cast" => {
                        let args = op.args.as_ref().unwrap();
                        let view = locals[&args[0]];
                        let format = locals[&args[1]];
                        let shape = locals[&args[2]];
                        let has_shape = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(view));
                        func.instruction(&Instruction::LocalGet(format));
                        func.instruction(&Instruction::LocalGet(shape));
                        func.instruction(&Instruction::LocalGet(has_shape));
                        emit_call(func, reloc_enabled, import_ids["memoryview_cast"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_new" => {
                        let args = op.args.as_ref().unwrap();
                        let rows = locals[&args[0]];
                        let cols = locals[&args[1]];
                        let init = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(rows));
                        func.instruction(&Instruction::LocalGet(cols));
                        func.instruction(&Instruction::LocalGet(init));
                        emit_call(func, reloc_enabled, import_ids["buffer2d_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_get" => {
                        let args = op.args.as_ref().unwrap();
                        let buf = locals[&args[0]];
                        let row = locals[&args[1]];
                        let col = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(buf));
                        func.instruction(&Instruction::LocalGet(row));
                        func.instruction(&Instruction::LocalGet(col));
                        emit_call(func, reloc_enabled, import_ids["buffer2d_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_set" => {
                        let args = op.args.as_ref().unwrap();
                        let buf = locals[&args[0]];
                        let row = locals[&args[1]];
                        let col = locals[&args[2]];
                        let val = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(buf));
                        func.instruction(&Instruction::LocalGet(row));
                        func.instruction(&Instruction::LocalGet(col));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["buffer2d_set"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["buffer2d_matmul"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "str_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["str_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "repr_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["repr_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ascii_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["ascii_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        let fields = locals[&args[1]];
                        let values = locals[&args[2]];
                        let flags = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(fields));
                        func.instruction(&Instruction::LocalGet(values));
                        func.instruction(&Instruction::LocalGet(flags));
                        emit_call(func, reloc_enabled, import_ids["dataclass_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_get" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["dataclass_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_set" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["dataclass_set"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(class_obj));
                        emit_call(func, reloc_enabled, import_ids["dataclass_set_class"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["class_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_set_base" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let base_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(base_bits));
                        emit_call(func, reloc_enabled, import_ids["class_set_base"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_apply_set_name" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["class_apply_set_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "super_new" => {
                        let args = op.args.as_ref().unwrap();
                        let type_bits = locals[&args[0]];
                        let obj_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(type_bits));
                        func.instruction(&Instruction::LocalGet(obj_bits));
                        emit_call(func, reloc_enabled, import_ids["super_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "builtin_type" => {
                        let args = op.args.as_ref().unwrap();
                        let tag = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(tag));
                        emit_call(func, reloc_enabled, import_ids["builtin_type"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "type_of" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["type_of"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_layout_version" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["class_layout_version"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_set_layout_version" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let version_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(version_bits));
                        emit_call(func, reloc_enabled, import_ids["class_set_layout_version"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                let res = locals[out];
                                func.instruction(&Instruction::LocalSet(res));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "isinstance" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(cls));
                        emit_call(func, reloc_enabled, import_ids["isinstance"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "issubclass" => {
                        let args = op.args.as_ref().unwrap();
                        let sub = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sub));
                        func.instruction(&Instruction::LocalGet(cls));
                        emit_call(func, reloc_enabled, import_ids["issubclass"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "object_new" => {
                        emit_call(func, reloc_enabled, import_ids["object_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "classmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["classmethod_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "staticmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["staticmethod_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "property_new" => {
                        let args = op.args.as_ref().unwrap();
                        let getter = locals[&args[0]];
                        let setter = locals[&args[1]];
                        let deleter = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(getter));
                        func.instruction(&Instruction::LocalGet(setter));
                        func.instruction(&Instruction::LocalGet(deleter));
                        emit_call(func, reloc_enabled, import_ids["property_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "object_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalGet(class_obj));
                        emit_call(func, reloc_enabled, import_ids["object_set_class"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["get_attr_ptr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "get_attr_generic_obj",
                        ));
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::I64Const(site_bits));
                        emit_call(func, reloc_enabled, import_ids["get_attr_object_ic"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_special_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["get_attr_special"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["set_attr_ptr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = *locals.get(&args[0]).unwrap_or_else(|| {
                            panic!(
                                "missing local {} in {} for {}",
                                args[0], func_ir.name, op.kind
                            )
                        });
                        let val = *locals.get(&args[1]).unwrap_or_else(|| {
                            panic!(
                                "missing local {} in {} for {}",
                                args[1], func_ir.name, op.kind
                            )
                        });
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["set_attr_object"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["del_attr_ptr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["del_attr_object"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["get_attr_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_name_default" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        let default_val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(default_val));
                        emit_call(func, reloc_enabled, import_ids["get_attr_name_default"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "has_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["has_attr_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["set_attr_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["del_attr_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "store" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_old = locals["__wasm_tmp1"];

                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_old));

                        func.instruction(&Instruction::LocalGet(tmp_old));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::I32Or);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_set"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref()
                            && out != "none"
                        {
                            func.instruction(&Instruction::I64Const(box_none()));
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_set"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "store_init" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];

                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_init"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref()
                            && out != "none"
                        {
                            func.instruction(&Instruction::I64Const(box_none()));
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_init"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "load" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_val = locals["__wasm_tmp1"];
                        let out = locals[op.out.as_ref().unwrap()];

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_val));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        emit_call(func, reloc_enabled, import_ids["object_field_get"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "closure_load" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let tmp_ptr = locals["__molt_tmp0"];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        emit_call(func, reloc_enabled, import_ids["closure_load"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "closure_store" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let tmp_ptr = locals["__molt_tmp0"];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(locals[&args[1]]));
                        emit_call(func, reloc_enabled, import_ids["closure_store"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "guarded_load" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_val = locals["__wasm_tmp1"];
                        let out = locals[op.out.as_ref().unwrap()];

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_val));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        emit_call(func, reloc_enabled, import_ids["object_field_get"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "guarded_field_get" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        let tmp_ptr = locals["__wasm_tmp0"];
                        let tmp_val = locals["__wasm_tmp1"];
                        let guard_val = locals["__molt_tmp0"];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        func.instruction(&Instruction::LocalSet(guard_val));

                        func.instruction(&Instruction::LocalGet(guard_val));
                        func.instruction(&Instruction::I64Const(box_bool(1)));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_val));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["guarded_field_get_ptr"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                        func.instruction(&Instruction::End);
                    }
                    "guarded_field_set" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        let val = locals[&args[3]];
                        let tmp_ptr = locals["__wasm_tmp0"];
                        let tmp_old = locals["__wasm_tmp1"];
                        let guard_val = locals["__molt_tmp0"];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        func.instruction(&Instruction::LocalSet(guard_val));

                        func.instruction(&Instruction::LocalGet(guard_val));
                        func.instruction(&Instruction::I64Const(box_bool(1)));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_old));

                        func.instruction(&Instruction::LocalGet(tmp_old));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::I32Or);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_set_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref()
                            && out != "none"
                        {
                            func.instruction(&Instruction::I64Const(box_none()));
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["guarded_field_set_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "guarded_field_init" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        let val = locals[&args[3]];
                        let tmp_ptr = locals["__wasm_tmp0"];
                        let guard_val = locals["__molt_tmp0"];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        func.instruction(&Instruction::LocalSet(guard_val));

                        func.instruction(&Instruction::LocalGet(guard_val));
                        func.instruction(&Instruction::I64Const(box_bool(1)));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_init_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref()
                            && out != "none"
                        {
                            func.instruction(&Instruction::I64Const(box_none()));
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["guarded_field_init_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "state_switch" => {}
                    "state_transition" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let slot_bits = args.get(1).map(|name| locals[name]);
                        let out = locals[op.out.as_ref().unwrap()];
                        let self_ptr = locals["__molt_tmp0"];
                        func.instruction(&Instruction::LocalGet(0));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(self_ptr));
                        func.instruction(&Instruction::LocalGet(self_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_poll"]);
                        func.instruction(&Instruction::LocalSet(out));
                        if let Some(slot) = slot_bits {
                            func.instruction(&Instruction::LocalGet(self_ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(slot));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalGet(out));
                            emit_call(func, reloc_enabled, import_ids["closure_store"]);
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::LocalGet(self_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["sleep_register"]);
                        func.instruction(&Instruction::Drop);
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                    }
                    "call_async" => {
                        let payload_len = op.args.as_ref().map(|args| args.len()).unwrap_or(0);
                        let table_slot = func_map[op.s_value.as_ref().unwrap()];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::I64Const((payload_len * 8) as i64));
                        func.instruction(&Instruction::I64Const(TASK_KIND_FUTURE));
                        emit_call(func, reloc_enabled, import_ids["task_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        if let Some(args) = op.args.as_ref() {
                            for (idx, arg) in args.iter().enumerate() {
                                let arg_val = locals[arg];
                                func.instruction(&Instruction::LocalGet(res));
                                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                                func.instruction(&Instruction::I32Const((idx * 8) as i32));
                                func.instruction(&Instruction::I32Add);
                                func.instruction(&Instruction::LocalGet(arg_val));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                                func.instruction(&Instruction::LocalGet(arg_val));
                                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            }
                        }
                    }
                    "call" => {
                        let target_name = op.s_value.as_ref().unwrap();
                        let args_names = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        let func_idx = *func_indices
                            .get(target_name)
                            .expect("call target not found");
                        let bootstrap_call = func_idx == import_ids["runtime_init"];
                        if bootstrap_call {
                            for arg_name in args_names {
                                let arg = locals[arg_name];
                                func.instruction(&Instruction::LocalGet(arg));
                            }
                            emit_call(func, reloc_enabled, func_idx);
                            func.instruction(&Instruction::LocalSet(out));
                            continue;
                        }
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                        func.instruction(&Instruction::Drop);
                        for arg_name in args_names {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        emit_call(func, reloc_enabled, func_idx);
                        func.instruction(&Instruction::LocalSet(out));
                        emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                        func.instruction(&Instruction::Drop);
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(box_none()));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "call_internal" => {
                        let target_name = op.s_value.as_ref().unwrap();
                        let args_names = op.args.as_ref().unwrap();
                        let out_name = op.out.as_ref().unwrap();
                        let out = locals[out_name];
                        let func_idx = *func_indices
                            .get(target_name)
                            .expect("call_internal target not found");

                        // --- Tail call detection (WASM tail calls proposal §3.5) ---
                        // A call_internal is in tail position when:
                        //   1. The function is eligible (no exception handling)
                        //   2. The very next op is `ret`
                        //   3. The ret's var matches this call's output
                        //   4. There are no cleanup ops (dec_ref) between call and return
                        let is_tail_call = tail_call_eligible
                            && rel_idx + 1 < ops.len()
                            && ops[rel_idx + 1].kind == "ret"
                            && ops[rel_idx + 1].var.as_deref() == Some(out_name.as_str())
                            // Exclude calls to multi-return candidates: return_call
                            // would forward N values but the caller's type signature
                            // expects a single i64 return, causing an ABI mismatch.
                            && !multi_return_candidates.contains_key(target_name);

                        for arg_name in args_names {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }

                        if is_tail_call {
                            // Emit return_call: callee's return value becomes
                            // our return value without growing the WASM stack.
                            emit_return_call(func, reloc_enabled, func_idx);
                            tail_call_count.set(tail_call_count.get() + 1);
                            // Skip the next op (ret) since return_call subsumes it.
                            skip_next = true;
                            continue;
                        }

                        emit_call(func, reloc_enabled, func_idx);
                        // Multi-value return (Section 3.1): pop N results
                        // into dedicated locals for later tuple_index.
                        if multi_ret_call_vars.contains(out_name) {
                            let ret_count = multi_return_candidates[target_name];
                            for k in (0..ret_count).rev() {
                                let local_idx =
                                    multi_ret_call_locals[&(out_name.clone(), k as i64)];
                                func.instruction(&Instruction::LocalSet(local_idx));
                            }
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::LocalSet(out));
                        } else {
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "inc_ref" | "borrow" => {
                        let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
                        let src_name = args_names
                            .first()
                            .expect("inc_ref/borrow requires one source arg");
                        let src = locals[src_name];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            let out = locals[out_name];
                            func.instruction(&Instruction::LocalGet(src));
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "dec_ref" | "release" => {
                        let args_names = op.args.as_ref().expect("dec_ref/release args missing");
                        let src_name = args_names
                            .first()
                            .expect("dec_ref/release requires one source arg");
                        let src = locals[src_name];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            let out = locals[out_name];
                            func.instruction(&Instruction::I64Const(box_none()));
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "box" | "unbox" | "cast" | "widen" => {
                        let args_names = op.args.as_ref().expect("conversion args missing");
                        let src_name = args_names
                            .first()
                            .expect("conversion op requires one source arg");
                        let src = locals[src_name];
                        func.instruction(&Instruction::LocalGet(src));
                        if let Some(out_name) = op.out.as_ref() {
                            if out_name != "none" {
                                // Output aliases input bits — inc_ref to prevent
                                // use-after-free when the input name is dec_ref'd
                                // independently by tracking/check_exception cleanup.
                                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                                func.instruction(&Instruction::LocalGet(src));
                                let out = locals[out_name];
                                func.instruction(&Instruction::LocalSet(out));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "identity_alias" => {
                        let args_names = op.args.as_ref().expect("identity_alias args missing");
                        let src_name = args_names
                            .first()
                            .expect("identity_alias requires one source arg");
                        let src = locals[src_name];
                        func.instruction(&Instruction::LocalGet(src));
                        if let Some(out_name) = op.out.as_ref() {
                            if out_name != "none" {
                                // Same aliasing hazard as box/unbox/cast/widen above.
                                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                                func.instruction(&Instruction::LocalGet(src));
                                let out = locals[out_name];
                                func.instruction(&Instruction::LocalSet(out));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "call_guarded" => {
                        let target_name = op.s_value.as_ref().unwrap();
                        let args_names = op.args.as_ref().unwrap();
                        let callee_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let callargs_tmp = locals["__molt_tmp0"];
                        let tmp_ptr = locals["__molt_tmp1"];
                        let arity = args_names.len().saturating_sub(1);
                        let func_idx = *func_indices
                            .get(target_name)
                            .expect("call_guarded target not found");
                        let table_slot = func_map[target_name];
                        let table_idx = table_base + table_slot;
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        emit_call(func, reloc_enabled, import_ids["is_function_obj"]);
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        // callee is a function object: resolve and compare against expected target
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        // fast path: callee matches expected target
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                        func.instruction(&Instruction::Drop);
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        emit_call(func, reloc_enabled, func_idx);
                        func.instruction(&Instruction::LocalSet(out));
                        emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                        func.instruction(&Instruction::Drop);
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(box_none()));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);

                        // slow path: function object does not match expected target
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_guarded_slow_match_miss",
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);

                        // not a function object: fallback to call_bind
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_guarded_nonfunc",
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "func_new" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        let tramp_slot = trampoline_map[func_name];
                        let tramp_idx = table_base + tramp_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        emit_table_index_i64(func, reloc_enabled, tramp_idx);
                        func.instruction(&Instruction::I64Const(arity));
                        emit_call(func, reloc_enabled, import_ids["func_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "func_new_closure" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let closure_name = op
                            .args
                            .as_ref()
                            .and_then(|args| args.first())
                            .expect("func_new_closure expects closure arg");
                        let closure_bits = locals[closure_name];
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        let tramp_slot = trampoline_map[func_name];
                        let tramp_idx = table_base + tramp_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        emit_table_index_i64(func, reloc_enabled, tramp_idx);
                        func.instruction(&Instruction::I64Const(arity));
                        func.instruction(&Instruction::LocalGet(closure_bits));
                        emit_call(func, reloc_enabled, import_ids["func_new_closure"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "code_new" => {
                        let args = op.args.as_ref().unwrap();
                        let filename_bits = locals[&args[0]];
                        let name_bits = locals[&args[1]];
                        let firstlineno_bits = locals[&args[2]];
                        let linetable_bits = locals[&args[3]];
                        let varnames_bits = locals[&args[4]];
                        let argcount_bits = locals[&args[5]];
                        let posonlyargcount_bits = locals[&args[6]];
                        let kwonlyargcount_bits = locals[&args[7]];
                        func.instruction(&Instruction::LocalGet(filename_bits));
                        func.instruction(&Instruction::LocalGet(name_bits));
                        func.instruction(&Instruction::LocalGet(firstlineno_bits));
                        func.instruction(&Instruction::LocalGet(linetable_bits));
                        func.instruction(&Instruction::LocalGet(varnames_bits));
                        func.instruction(&Instruction::LocalGet(argcount_bits));
                        func.instruction(&Instruction::LocalGet(posonlyargcount_bits));
                        func.instruction(&Instruction::LocalGet(kwonlyargcount_bits));
                        emit_call(func, reloc_enabled, import_ids["code_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "code_slot_set" => {
                        let args = op.args.as_ref().unwrap();
                        let code_bits = locals[&args[0]];
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        func.instruction(&Instruction::LocalGet(code_bits));
                        emit_call(func, reloc_enabled, import_ids["code_slot_set"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "fn_ptr_code_set" => {
                        let args = op.args.as_ref().unwrap();
                        let code_bits = locals[&args[0]];
                        let func_name = op.s_value.as_ref().unwrap();
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::LocalGet(code_bits));
                        emit_call(func, reloc_enabled, import_ids["fn_ptr_code_set"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "asyncgen_locals_register" => {
                        let args = op.args.as_ref().unwrap();
                        let names_bits = locals[&args[0]];
                        let offsets_bits = locals[&args[1]];
                        let func_name = op.s_value.as_ref().unwrap();
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::LocalGet(names_bits));
                        func.instruction(&Instruction::LocalGet(offsets_bits));
                        emit_call(func, reloc_enabled, import_ids["asyncgen_locals_register"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "gen_locals_register" => {
                        let args = op.args.as_ref().unwrap();
                        let names_bits = locals[&args[0]];
                        let offsets_bits = locals[&args[1]];
                        let func_name = op.s_value.as_ref().unwrap();
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::LocalGet(names_bits));
                        func.instruction(&Instruction::LocalGet(offsets_bits));
                        emit_call(func, reloc_enabled, import_ids["gen_locals_register"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "code_slots_init" => {
                        let count = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(count));
                        emit_call(func, reloc_enabled, import_ids["code_slots_init"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "line" => {
                        let line = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(line));
                        emit_call(func, reloc_enabled, import_ids["trace_set_line"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "frame_locals_set" => {
                        let args = op.args.as_ref().expect("frame_locals_set args missing");
                        let dict_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict_bits));
                        emit_call(func, reloc_enabled, import_ids["frame_locals_set"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "builtin_func" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        let tramp_slot = trampoline_map[func_name];
                        let tramp_idx = table_base + tramp_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        emit_table_index_i64(func, reloc_enabled, tramp_idx);
                        func.instruction(&Instruction::I64Const(arity));
                        emit_call(func, reloc_enabled, import_ids["func_new_builtin"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "missing" => {
                        let out = locals[op.out.as_ref().unwrap()];
                        emit_call(func, reloc_enabled, import_ids["missing"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "function_closure_bits" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["function_closure_bits"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    "bound_method_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        let self_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(self_bits));
                        emit_call(func, reloc_enabled, import_ids["bound_method_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "call_func" | "invoke_ffi" => {
                        let args_names = op.args.as_ref().unwrap();
                        let func_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let callargs_tmp = locals["__molt_tmp0"];
                        let arity = args_names.len().saturating_sub(1);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        let invoke_bridge_lane =
                            op.kind == "invoke_ffi" && op.s_value.as_deref() == Some("bridge");
                        let call_site_label = if op.kind == "invoke_ffi" {
                            if invoke_bridge_lane {
                                "invoke_ffi_bridge"
                            } else {
                                "invoke_ffi_deopt"
                            }
                        } else {
                            "call_func"
                        };
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            call_site_label,
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        if op.kind == "invoke_ffi" {
                            let require_bridge_cap = if invoke_bridge_lane { 1 } else { 0 };
                            func.instruction(&Instruction::I64Const(box_bool(require_bridge_cap)));
                            emit_call(func, reloc_enabled, import_ids["invoke_ffi_ic"]);
                        } else {
                            emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                        }
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "call_bind" | "call_indirect" => {
                        let args_names = op.args.as_ref().unwrap();
                        let func_bits = locals[&args_names[0]];
                        let builder_ptr = locals[&args_names[1]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let call_site_label = if op.kind == "call_indirect" {
                            "call_indirect"
                        } else {
                            "call_bind"
                        };
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            call_site_label,
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        if op.kind == "call_indirect" {
                            emit_call(func, reloc_enabled, import_ids["call_indirect_ic"]);
                        } else {
                            emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                        }
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "call_method" => {
                        let args_names = op.args.as_ref().unwrap();
                        let method_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let callargs_tmp = locals["__molt_tmp0"];
                        let arity = args_names.len().saturating_sub(1);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_method",
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "chan_new" => {
                        let args = op.args.as_ref().unwrap();
                        let cap = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cap));
                        emit_call(func, reloc_enabled, import_ids["chan_new"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "chan_drop" => {
                        let args = op.args.as_ref().unwrap();
                        let chan = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(chan));
                        emit_call(func, reloc_enabled, import_ids["chan_drop"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "module_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_cache_get" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_cache_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_import" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_import"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_cache_set" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        let module = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(module));
                        emit_call(func, reloc_enabled, import_ids["module_cache_set"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_get_attr" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_attr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_get_global" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_global"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_del_global" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_del_global"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                let res = locals[out];
                                func.instruction(&Instruction::LocalSet(res));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "module_get_name" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_set_attr" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["module_set_attr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_import_star" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        let dst = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalGet(dst));
                        emit_call(func, reloc_enabled, import_ids["module_import_star"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "alloc_task" => {
                        let total = op.value.unwrap_or(0);
                        let task_kind = op.task_kind.as_deref().unwrap_or("future");
                        let (kind_bits, payload_base) = match task_kind {
                            "generator" => (TASK_KIND_GENERATOR, GEN_CONTROL_SIZE),
                            "future" => (TASK_KIND_FUTURE, 0),
                            "coroutine" => (TASK_KIND_COROUTINE, 0),
                            _ => panic!("unknown task kind: {task_kind}"),
                        };
                        let table_slot = func_map[op.s_value.as_ref().unwrap()];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::I64Const(total));
                        func.instruction(&Instruction::I64Const(kind_bits));
                        emit_call(func, reloc_enabled, import_ids["task_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        // Resolve the task handle pointer once and cache in a
                        // local, mirroring the trampoline codepath pattern
                        // (WASM_OPTIMIZATION_PLAN Section 3.3).
                        let has_args = op.args.as_ref().map_or(false, |a| !a.is_empty());
                        if payload_base > 0 || has_args {
                            let resolve_local = locals["__wasm_alloc_resolve"];
                            func.instruction(&Instruction::LocalGet(res));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::LocalSet(resolve_local));
                            if payload_base > 0 {
                                func.instruction(&Instruction::LocalGet(resolve_local)); // dest
                                func.instruction(&Instruction::I32Const(0)); // fill value
                                func.instruction(&Instruction::I32Const(payload_base)); // byte count
                                func.instruction(&Instruction::MemoryFill(0));
                            }
                        }
                        if let Some(args) = op.args.as_ref() {
                            let resolve_local = locals["__wasm_alloc_resolve"];
                            for (i, name) in args.iter().enumerate() {
                                let arg_local = locals[name];
                                func.instruction(&Instruction::LocalGet(resolve_local));
                                func.instruction(&Instruction::I32Const(
                                    payload_base + (i as i32) * 8,
                                ));
                                func.instruction(&Instruction::I32Add);
                                func.instruction(&Instruction::LocalGet(arg_local));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                                func.instruction(&Instruction::LocalGet(arg_local));
                                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            }
                        }
                        if matches!(task_kind, "future" | "coroutine") {
                            func.instruction(&Instruction::LocalGet(res));
                            emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                            emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "state_yield" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(0));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        let pair = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(pair));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalGet(pair));
                            func.instruction(&Instruction::LocalSet(locals[out]));
                            func.instruction(&Instruction::LocalGet(locals[out]));
                        } else {
                            func.instruction(&Instruction::LocalGet(pair));
                        }
                        func.instruction(&Instruction::Return);
                    }
                    "context_null" => {
                        let args = op.args.as_ref().unwrap();
                        let payload = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(payload));
                        emit_call(func, reloc_enabled, import_ids["context_null"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_enter" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        emit_call(func, reloc_enabled, import_ids["context_enter"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_exit" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_exit"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_unwind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_unwind"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_depth" => {
                        emit_call(func, reloc_enabled, import_ids["context_depth"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_unwind_to" => {
                        let args = op.args.as_ref().unwrap();
                        let depth = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(depth));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_unwind_to"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_closing" => {
                        let args = op.args.as_ref().unwrap();
                        let payload = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(payload));
                        emit_call(func, reloc_enabled, import_ids["context_closing"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_push" => {
                        if native_eh_enabled {
                            // Native EH: no-op; WASM runtime manages handler stack.
                            func.instruction(&Instruction::I64Const(box_none()));
                        } else {
                            emit_call(func, reloc_enabled, import_ids["exception_push"]);
                        }
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_pop" => {
                        if native_eh_enabled {
                            // Native EH: no-op; handler popped when try_table ends.
                            func.instruction(&Instruction::I64Const(box_none()));
                        } else {
                            emit_call(func, reloc_enabled, import_ids["exception_pop"]);
                        }
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_stack_clear" => {
                        emit_call(func, reloc_enabled, import_ids["exception_stack_clear"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_last" => {
                        emit_call(func, reloc_enabled, import_ids["exception_last"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_new" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        let args_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(kind));
                        func.instruction(&Instruction::LocalGet(args_bits));
                        emit_call(func, reloc_enabled, import_ids["exception_new"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_new_from_class" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let args_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(args_bits));
                        emit_call(func, reloc_enabled, import_ids["exception_new_from_class"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exceptiongroup_match" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let matcher = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(matcher));
                        emit_call(func, reloc_enabled, import_ids["exceptiongroup_match"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exceptiongroup_combine" => {
                        let args = op.args.as_ref().unwrap();
                        let items = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(items));
                        emit_call(func, reloc_enabled, import_ids["exceptiongroup_combine"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_clear" => {
                        emit_call(func, reloc_enabled, import_ids["exception_clear"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_kind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_kind"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_class" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(kind));
                        emit_call(func, reloc_enabled, import_ids["exception_class"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_message" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_message"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_set_cause" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let cause = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(cause));
                        emit_call(func, reloc_enabled, import_ids["exception_set_cause"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_set_value" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let value = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(value));
                        emit_call(func, reloc_enabled, import_ids["exception_set_value"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_context_set" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_context_set"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_set_last" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_set_last"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "raise" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        if native_eh_enabled {
                            // Native EH: call host raise to register the exception
                            // (traceback, __context__), then throw via WASM EH.
                            emit_call(func, reloc_enabled, import_ids["raise"]);
                            func.instruction(&Instruction::Drop);
                            func.instruction(&Instruction::LocalGet(exc));
                            func.instruction(&Instruction::Throw(TAG_EXCEPTION_INDEX));
                        } else {
                            emit_call(func, reloc_enabled, import_ids["raise"]);
                            func.instruction(&Instruction::LocalSet(
                                locals[op.out.as_ref().unwrap()],
                            ));
                        }
                    }
                    "bridge_unavailable" => {
                        let args = op.args.as_ref().unwrap();
                        let msg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(msg));
                        emit_call(func, reloc_enabled, import_ids["bridge_unavailable"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_open" => {
                        let args = op.args.as_ref().unwrap();
                        let path = locals[&args[0]];
                        let mode = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(path));
                        func.instruction(&Instruction::LocalGet(mode));
                        emit_call(func, reloc_enabled, import_ids["file_open"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_read" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let size = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(size));
                        emit_call(func, reloc_enabled, import_ids["file_read"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_write" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let data = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(data));
                        emit_call(func, reloc_enabled, import_ids["file_write"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_close" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(handle));
                        emit_call(func, reloc_enabled, import_ids["file_close"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_flush" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(handle));
                        emit_call(func, reloc_enabled, import_ids["file_flush"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_new" => {
                        let args = op.args.as_ref().unwrap();
                        let parent = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(parent));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_new"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_clone" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_clone"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_drop" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_drop"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_cancel" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_cancel"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "future_cancel" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_cancel"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "future_cancel_msg" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let msg = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(msg));
                        emit_call(func, reloc_enabled, import_ids["future_cancel_msg"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "future_cancel_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_cancel_clear"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "promise_new" => {
                        emit_call(func, reloc_enabled, import_ids["promise_new"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "promise_set_result" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let result = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(result));
                        emit_call(func, reloc_enabled, import_ids["promise_set_result"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "promise_set_exception" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["promise_set_exception"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "thread_submit" => {
                        let args = op.args.as_ref().unwrap();
                        let callable = locals[&args[0]];
                        let call_args = locals[&args[1]];
                        let call_kwargs = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(callable));
                        func.instruction(&Instruction::LocalGet(call_args));
                        func.instruction(&Instruction::LocalGet(call_kwargs));
                        emit_call(func, reloc_enabled, import_ids["thread_submit"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "task_register_token_owned" => {
                        let args = op.args.as_ref().unwrap();
                        let task = locals[&args[0]];
                        let token = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(task));
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "spawn" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        emit_call(func, reloc_enabled, import_ids["spawn"]);
                    }
                    "cancel_token_is_cancelled" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_is_cancelled"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_set_current" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_set_current"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_get_current" => {
                        emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancelled" => {
                        emit_call(func, reloc_enabled, import_ids["cancelled"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_current" => {
                        emit_call(func, reloc_enabled, import_ids["cancel_current"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "block_on" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        emit_call(func, reloc_enabled, import_ids["block_on"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "ret" => {
                        let ret_var = op.var.as_ref();
                        // Multi-value return (Section 3.1): push individual
                        // __multi_ret_N locals instead of the tuple handle.
                        if is_multi_return_callee.is_some()
                            && ret_var.map_or(false, |v| multi_ret_tuple_vars.contains(v))
                            && !multi_ret_locals.is_empty()
                        {
                            for &local_idx in &multi_ret_locals {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            }
                        } else {
                            let ret_local = ret_var.and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                eprintln!(
                                    "WASM lowering warning: missing return local in {} op {} (var={:?}); returning None",
                                    func_ir.name, op_idx, op.var
                                );
                                func.instruction(&Instruction::I64Const(box_none()));
                            }
                        }
                        func.instruction(&Instruction::Return);
                    }
                    "ret_void" => {
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::Return);
                    }
                    "jump" => {
                        let target = op.value.expect("jump missing label");
                        let depth = label_depths
                            .get(&target)
                            .map(|idx| control_stack.len().saturating_sub(1 + idx))
                            .unwrap_or_else(|| {
                                panic!("jump target {} missing label block", target)
                            });
                        func.instruction(&Instruction::Br(depth as u32));
                    }
                    "if" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        control_stack.push(ControlKind::If);
                    }
                    "label" => {
                        if let Some(label_id) = op.value
                            && let Some(top) = label_stack.last().copied()
                            && top == label_id
                        {
                            label_stack.pop();
                            label_depths.remove(&label_id);
                            func.instruction(&Instruction::End);
                            control_stack.pop();
                        }
                    }
                    "else" => {
                        func.instruction(&Instruction::Else);
                    }
                    "end_if" => {
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                    }
                    "loop_start" => {
                        func.instruction(&Instruction::Block(BlockType::Empty));
                        func.instruction(&Instruction::Loop(BlockType::Empty));
                        control_stack.push(ControlKind::Block);
                        control_stack.push(ControlKind::Loop);
                    }
                    "loop_index_start" => {
                        let args = op.args.as_ref().unwrap();
                        let start = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalSet(out));
                        // Block+Loop already emitted by preceding loop_start;
                        // do NOT push a second Block+Loop pair here.
                    }
                    "loop_index_next" => {
                        let args = op.args.as_ref().unwrap();
                        let next_idx = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(next_idx));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "loop_break_if_true" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        // Find depth to the enclosing Block that wraps the Loop.
                        let mut depth = 0u32;
                        let mut found_loop = false;
                        for entry in control_stack.iter().rev() {
                            match entry {
                                ControlKind::Block if found_loop => break,
                                ControlKind::Loop => {
                                    found_loop = true;
                                }
                                _ => {}
                            }
                            depth += 1;
                        }
                        func.instruction(&Instruction::BrIf(depth));
                    }
                    "loop_break_if_false" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Eq);
                        // Find depth to the enclosing Block that wraps the Loop.
                        let mut depth = 0u32;
                        let mut found_loop = false;
                        for entry in control_stack.iter().rev() {
                            match entry {
                                ControlKind::Block if found_loop => break,
                                ControlKind::Loop => {
                                    found_loop = true;
                                }
                                _ => {}
                            }
                            depth += 1;
                        }
                        func.instruction(&Instruction::BrIf(depth));
                    }
                    "loop_break" => {
                        // Find depth to the enclosing Block that wraps the Loop.
                        // The loop structure is Block { Loop { ... } }, so we
                        // need to find the Block that immediately precedes
                        // the innermost Loop on the control stack.
                        let mut depth = 0u32;
                        let mut found_loop = false;
                        for entry in control_stack.iter().rev() {
                            match entry {
                                ControlKind::Block if found_loop => break,
                                ControlKind::Loop => {
                                    found_loop = true;
                                }
                                _ => {}
                            }
                            depth += 1;
                        }
                        func.instruction(&Instruction::Br(depth));
                    }
                    "loop_continue" => {
                        // Find depth to the innermost Loop on the control stack.
                        let mut depth = 0u32;
                        for entry in control_stack.iter().rev() {
                            if matches!(entry, ControlKind::Loop) {
                                break;
                            }
                            depth += 1;
                        }
                        func.instruction(&Instruction::Br(depth));
                    }
                    "loop_end" => {
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                        control_stack.pop();
                    }
                    "try_start" => {
                        if native_eh_enabled {
                            // Native EH: two-level block for try_table:
                            //   block $catch_dest (result i64)
                            //     try_table (catch $molt_exception $catch_dest)
                            //       ... body ...
                            //     end
                            //     i64.const <box_none>  ;; normal path sentinel
                            //   end
                            //   ;; catch: exception handle on stack
                            func.instruction(&Instruction::Block(BlockType::Result(ValType::I64)));
                            control_stack.push(ControlKind::Block);
                            func.instruction(&Instruction::TryTable(
                                BlockType::Empty,
                                Cow::Borrowed(&[Catch::One {
                                    tag: TAG_EXCEPTION_INDEX,
                                    label: 0,
                                }]),
                            ));
                            control_stack.push(ControlKind::Try);
                            try_stack.push(control_stack.len() - 1);
                        } else {
                            func.instruction(&Instruction::Block(BlockType::Empty));
                            control_stack.push(ControlKind::Try);
                            try_stack.push(control_stack.len() - 1);
                        }
                    }
                    "try_end" => {
                        if native_eh_enabled {
                            // Close try_table
                            func.instruction(&Instruction::End);
                            control_stack.pop();
                            try_stack.pop();
                            // Normal path: push None sentinel for outer block result
                            func.instruction(&Instruction::I64Const(box_none()));
                            // Close outer catch-destination block
                            func.instruction(&Instruction::End);
                            control_stack.pop();
                            // Drop the i64 result (exception handle or sentinel)
                            func.instruction(&Instruction::Drop);
                        } else {
                            func.instruction(&Instruction::End);
                            control_stack.pop();
                            try_stack.pop();
                        }
                    }
                    "check_exception" => {
                        if native_eh_enabled {
                            // Native EH: no-op; WASM catches automatically.
                        } else if let Some(&try_index) = try_stack.last() {
                            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            let depth = control_stack.len().saturating_sub(1 + try_index);
                            func.instruction(&Instruction::BrIf(depth as u32));
                        }
                    }
                    // ---------------------------------------------------------------
                    // memory_copy: bulk linear-memory copy (WASM 2.0 bulk-memory op)
                    //
                    // IR signature:  memory_copy(dst, src, len)
                    //   dst, src  – i64 boxed integers holding i32 linear-memory byte
                    //               offsets (e.g. from handle_resolve)
                    //   len       – i64 boxed integer holding the byte count
                    //
                    // Emits:  memory.copy  (dst_mem=0, src_mem=0)
                    //         stack: [dst:i32, src:i32, len:i32]
                    //
                    // This intrinsic enables the IR to emit efficient buffer-to-buffer
                    // copies without round-tripping through host imports.  See
                    // WASM_OPTIMIZATION_PLAN.md Section 3.3.
                    // ---------------------------------------------------------------
                    "memory_copy" => {
                        let args = op.args.as_ref().unwrap();
                        debug_assert!(
                            args.len() == 3,
                            "memory_copy requires exactly 3 args (dst, src, len)"
                        );
                        let dst = locals[&args[0]];
                        let src = locals[&args[1]];
                        let len = locals[&args[2]];
                        // Unbox each i64 value to i32 for the memory.copy instruction.
                        func.instruction(&Instruction::LocalGet(dst));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::MemoryCopy {
                            src_mem: 0,
                            dst_mem: 0,
                        });
                    }
                    _ => {}
                }

                // --- Peephole: invalidate known_raw_ints tracking ---
                // Control-flow ops make compile-time value tracking
                // unreliable across branches; clear everything.
                match op.kind.as_str() {
                    "if"
                    | "else"
                    | "end_if"
                    | "loop_start"
                    | "loop_index_start"
                    | "loop_break"
                    | "loop_break_if_true"
                    | "loop_break_if_false"
                    | "loop_continue"
                    | "label"
                    | "jump"
                    | "state_switch"
                    | "state_transition"
                    | "state_yield"
                    | "chan_send_yield"
                    | "chan_recv_yield"
                    | "try_start"
                    | "try_end"
                    | "check_exception"
                    | "loop_end"
                    | "ret"
                    | "ret_void" => {
                        known_raw_ints.clear();
                    }
                    // `const` already recorded its value above; skip invalidation.
                    "const" => {}
                    // All other ops: invalidate only the output local (if any),
                    // since only that local's value changed.
                    _ => {
                        if let Some(ref out) = op.out {
                            if let Some(&out_idx) = locals.get(out.as_str()) {
                                known_raw_ints.remove(&out_idx);
                            }
                        }
                    }
                }
            }
        };

        if stateful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for stateful wasm");
            let self_ptr_local = self_ptr_local.expect("self ptr local missing for stateful wasm");
            let self_param = *locals
                .get("self_param")
                .expect("self_param missing for stateful wasm");
            let self_local = *locals
                .get("self")
                .expect("self local missing for stateful wasm");
            let op_count = func_ir.ops.len();
            let (block_starts, _) = dispatch_blocks
                .as_ref()
                .expect("dispatch blocks missing for stateful wasm");
            let block_count = block_starts.len();
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for stateful wasm");
            let dispatch_control_maps = dispatch_control_maps
                .as_ref()
                .expect("dispatch control maps missing for stateful wasm");
            let label_to_index = &dispatch_control_maps.label_to_index;
            let else_for_if = &dispatch_control_maps.else_for_if;
            let end_for_if = &dispatch_control_maps.end_for_if;
            let end_for_else = &dispatch_control_maps.end_for_else;
            let loop_continue_target = &dispatch_control_maps.loop_continue_target;
            let loop_break_target = &dispatch_control_maps.loop_break_target;
            let (state_map, const_ints, state_remap_table) = state_resume_maps
                .as_ref()
                .expect("state resume maps missing for stateful wasm");
            let state_remap_table_entries = state_remap_table.as_ref().map(|(entries, _)| *entries);
            let sparse_state_remap_entries = state_remap_table_entries
                .is_none()
                .then(|| build_sparse_state_remap_entries(state_map));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::LocalSet(self_ptr_local));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
            func.instruction(&Instruction::I64Or);
            func.instruction(&Instruction::LocalSet(self_local));

            func.instruction(&Instruction::LocalGet(self_ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64LtS);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(-1));
            func.instruction(&Instruction::I64Xor);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            if let Some(remap_entries) = state_remap_table_entries {
                let remap_base_local = state_remap_base_local
                    .expect("state remap base local missing for stateful wasm");
                let remap_value_local = state_remap_value_local
                    .expect("state remap value local missing for stateful wasm");
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(remap_entries));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(remap_base_local));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::I32Const(8));
                func.instruction(&Instruction::I32Mul);
                func.instruction(&Instruction::I32Add);
                func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::LocalSet(remap_value_local));
                func.instruction(&Instruction::LocalGet(remap_value_local));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64GeS);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(remap_value_local));
                func.instruction(&Instruction::LocalSet(state_local));
                func.instruction(&Instruction::End);
                func.instruction(&Instruction::End);
            } else {
                emit_sparse_state_remap_lookup(
                    func,
                    state_local,
                    sparse_state_remap_entries
                        .as_deref()
                        .expect("sparse state remap entries missing for stateful wasm"),
                );
            }
            func.instruction(&Instruction::End);

            let dispatch_depths: Vec<u32> = (0..block_count)
                .map(|idx| (block_count - 1 - idx) as u32)
                .collect();

            let return_local = return_local.expect("stateful/jumpful missing return local");
            func.instruction(&Instruction::Block(BlockType::Empty));
            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..block_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(op_count as i64));
            func.instruction(&Instruction::I64GeU);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::I64Const(block_count as i64));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(block_map_base_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(4));
            func.instruction(&Instruction::I32Mul);
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                align: 2,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
            func.instruction(&Instruction::End);

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();

            for (block_idx, start) in block_starts.iter().enumerate() {
                let end = block_starts.get(block_idx + 1).copied().unwrap_or(op_count);
                let depth = dispatch_depths[block_idx];
                let mut block_terminated = false;

                for idx in *start..end {
                    let op = &func_ir.ops[idx];
                    match op.kind.as_str() {
                        "state_switch" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "aiter" => {
                            let args = op.args.as_ref().unwrap();
                            let iter = locals[&args[0]];
                            func.instruction(&Instruction::LocalGet(iter));
                            emit_call(func, reloc_enabled, import_ids["aiter"]);
                            func.instruction(&Instruction::LocalSet(
                                locals[op.out.as_ref().unwrap()],
                            ));
                        }
                        "state_transition" => {
                            let args = op.args.as_ref().unwrap();
                            let future = locals[&args[0]];
                            let (slot_bits, pending_state) = if args.len() == 2 {
                                (None, locals[&args[1]])
                            } else {
                                (Some(locals[&args[1]]), locals[&args[2]])
                            };
                            let pending_state_name =
                                if args.len() == 2 { &args[1] } else { &args[2] };
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let out = locals[op.out.as_ref().unwrap()];
                            let next_block = idx + 1;
                            let return_depth = depth + 2;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(future));
                            emit_call(func, reloc_enabled, import_ids["future_poll"]);
                            func.instruction(&Instruction::LocalSet(out));
                            // Store pending return value before the
                            // conditional so the If block does not
                            // leave values on the stack.
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::LocalSet(return_local));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(future));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["sleep_register"]);
                            func.instruction(&Instruction::Drop);
                            func.instruction(&Instruction::Br(return_depth));
                            func.instruction(&Instruction::End);
                            if let Some(slot) = slot_bits {
                                func.instruction(&Instruction::LocalGet(self_ptr_local));
                                func.instruction(&Instruction::I32WrapI64);
                                func.instruction(&Instruction::LocalGet(slot));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                                func.instruction(&Instruction::LocalGet(out));
                                emit_call(func, reloc_enabled, import_ids["closure_store"]);
                                func.instruction(&Instruction::Drop);
                            }
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "state_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let pair = locals[&args[0]];
                            let resume_state_id = op.value.unwrap();
                            let resume_encoded = state_map
                                .get(&resume_state_id)
                                .copied()
                                .map(|idx| !(idx as i64));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            if let Some(encoded) = resume_encoded {
                                func.instruction(&Instruction::I64Const(encoded));
                            } else {
                                func.instruction(&Instruction::I64Const(resume_state_id));
                            }
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(pair));
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            func.instruction(&Instruction::LocalGet(pair));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "chan_send_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let chan = locals[&args[0]];
                            let val = locals[&args[1]];
                            let pending_state = locals[&args[2]];
                            let pending_state_name = &args[2];
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(chan));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["chan_send"]);
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "chan_recv_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let chan = locals[&args[0]];
                            let pending_state = locals[&args[1]];
                            let pending_state_name = &args[1];
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(chan));
                            emit_call(func, reloc_enabled, import_ids["chan_recv"]);
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let else_idx = else_for_if.get(&idx).copied();
                            let Some(end_idx) = end_for_if.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: malformed if without end_if in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let false_target = if let Some(else_pos) = else_idx {
                                else_pos + 1
                            } else {
                                end_idx + 1
                            };
                            let true_block = idx + 1;
                            let false_block = false_target;
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(true_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(false_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "else" => {
                            let Some(end_idx) = end_for_else.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: malformed else without end_if in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "end_if" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_start" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_index_start" => {
                            let args = op.args.as_ref().unwrap();
                            let start = locals[&args[0]];
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(start));
                            func.instruction(&Instruction::LocalSet(out));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_break_if_true" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let Some(end_idx) = loop_break_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_break_if_true without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_false" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let Some(end_idx) = loop_break_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_break_if_false without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break" => {
                            let Some(end_idx) = loop_break_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_break without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_continue" => {
                            let Some(start_idx) = loop_continue_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_continue without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let start_block = start_idx + 1;
                            func.instruction(&Instruction::I64Const(start_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_end" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "jump" => {
                            let target_label = op.value.expect("jump missing label");
                            let Some(target_idx) = label_to_index.get(&target_label).copied()
                            else {
                                eprintln!(
                                    "WASM lowering warning: unknown jump label {} in {} at op {}; falling through",
                                    target_label, func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let target_block = target_idx;
                            func.instruction(&Instruction::I64Const(target_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "try_start" | "try_end" | "label" | "state_label" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "check_exception" => {
                            if native_eh_enabled {
                                // Native EH: skip polling; fall through to next state.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else {
                                let Some(target_label) = op.value else {
                                    eprintln!(
                                        "WASM lowering warning: check_exception missing label in {} at op {}; falling through",
                                        func_ir.name, idx
                                    );
                                    let next_block = idx + 1;
                                    func.instruction(&Instruction::I64Const(next_block as i64));
                                    func.instruction(&Instruction::LocalSet(state_local));
                                    func.instruction(&Instruction::Br(depth));
                                    block_terminated = true;
                                    continue;
                                };
                                let Some(target_idx) = label_to_index.get(&target_label).copied()
                                else {
                                    eprintln!(
                                        "WASM lowering warning: unknown check_exception label {} in {} at op {}; falling through",
                                        target_label, func_ir.name, idx
                                    );
                                    let next_block = idx + 1;
                                    func.instruction(&Instruction::I64Const(next_block as i64));
                                    func.instruction(&Instruction::LocalSet(state_local));
                                    func.instruction(&Instruction::Br(depth));
                                    block_terminated = true;
                                    continue;
                                };
                                let target_block = target_idx;
                                let next_block = idx + 1;
                                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                                func.instruction(&Instruction::I64Const(0));
                                func.instruction(&Instruction::I64Ne);
                                func.instruction(&Instruction::If(BlockType::Empty));
                                func.instruction(&Instruction::I64Const(target_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::Else);
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::End);
                                block_terminated = true;
                            }
                        }
                        "ret" => {
                            let ret_local =
                                op.var.as_ref().and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                eprintln!(
                                    "WASM lowering warning: missing state-machine return local in {} op {} (var={:?}); returning None",
                                    func_ir.name, idx, op.var
                                );
                                func.instruction(&Instruction::I64Const(box_none()));
                            }
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "ret_void" => {
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        _ => {
                            emit_ops(
                                func,
                                std::slice::from_ref(op),
                                &mut scratch_control,
                                &mut scratch_try,
                                &mut label_stack,
                                &mut label_depths,
                                idx,
                            );
                        }
                    }
                    if block_terminated {
                        break;
                    }
                }

                let next_state = end;
                if !block_terminated {
                    func.instruction(&Instruction::I64Const(next_state as i64));
                    func.instruction(&Instruction::LocalSet(state_local));
                }
                func.instruction(&Instruction::Br(depth));

                if block_idx + 1 < block_count {
                    func.instruction(&Instruction::End);
                }
            }

            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::LocalSet(return_local));
            func.instruction(&Instruction::End);
            func.instruction(&Instruction::LocalGet(return_local));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else if jumpful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for jumpful wasm");
            let op_count = func_ir.ops.len();
            let (block_starts, _) = dispatch_blocks
                .as_ref()
                .expect("dispatch blocks missing for jumpful wasm");
            let block_count = block_starts.len();
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for jumpful wasm");
            let dispatch_control_maps = dispatch_control_maps
                .as_ref()
                .expect("dispatch control maps missing for jumpful wasm");
            let label_to_index = &dispatch_control_maps.label_to_index;
            let else_for_if = &dispatch_control_maps.else_for_if;
            let end_for_if = &dispatch_control_maps.end_for_if;
            let end_for_else = &dispatch_control_maps.end_for_else;
            let loop_continue_target = &dispatch_control_maps.loop_continue_target;
            let loop_break_target = &dispatch_control_maps.loop_break_target;

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();
            let mut label_stack: Vec<i64> = Vec::new();
            let mut label_depths: HashMap<i64, usize> = HashMap::new();

            let dispatch_depths: Vec<u32> = (0..block_count)
                .map(|idx| (block_count - 1 - idx) as u32)
                .collect();

            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::LocalSet(state_local));

            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..block_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(op_count as i64));
            func.instruction(&Instruction::I64GeU);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::I64Const(block_count as i64));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(block_map_base_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(4));
            func.instruction(&Instruction::I32Mul);
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                align: 2,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
            func.instruction(&Instruction::End);

            for (block_idx, start) in block_starts.iter().enumerate() {
                let end = block_starts.get(block_idx + 1).copied().unwrap_or(op_count);
                let depth = dispatch_depths[block_idx];
                let mut block_terminated = false;

                for idx in *start..end {
                    let op = &func_ir.ops[idx];
                    match op.kind.as_str() {
                        "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                        | "chan_recv_yield" => {
                            eprintln!(
                                "WASM lowering warning: jumpful path hit stateful op {} in {} at op {}; falling through",
                                op.kind, func_ir.name, idx
                            );
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                            continue;
                        }
                        "if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let else_idx = else_for_if.get(&idx).copied();
                            let Some(end_idx) = end_for_if.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: malformed if without end_if in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let false_target = if let Some(else_pos) = else_idx {
                                else_pos + 1
                            } else {
                                end_idx + 1
                            };
                            let true_block = idx + 1;
                            let false_block = false_target;
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(true_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(false_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "else" => {
                            let Some(end_idx) = end_for_else.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: malformed else without end_if in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "end_if" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_start" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_index_start" => {
                            let args = op.args.as_ref().unwrap();
                            let start = locals[&args[0]];
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(start));
                            func.instruction(&Instruction::LocalSet(out));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_break_if_true" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let Some(end_idx) = loop_break_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_break_if_true without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_false" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let Some(end_idx) = loop_break_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_break_if_false without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break" => {
                            let Some(end_idx) = loop_break_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_break without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_continue" => {
                            let Some(start_idx) = loop_continue_target.get(&idx).copied() else {
                                eprintln!(
                                    "WASM lowering warning: loop_continue without loop in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let start_block = start_idx + 1;
                            func.instruction(&Instruction::I64Const(start_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_end" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "jump" => {
                            let Some(target_label) = op.value else {
                                eprintln!(
                                    "WASM lowering warning: jump missing label in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let Some(target_idx) = label_to_index.get(&target_label).copied()
                            else {
                                eprintln!(
                                    "WASM lowering warning: unknown jump label {} in {} at op {}; falling through",
                                    target_label, func_ir.name, idx
                                );
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                                continue;
                            };
                            let target_block = target_idx;
                            func.instruction(&Instruction::I64Const(target_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "try_start" | "try_end" | "label" | "state_label" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "check_exception" => {
                            if native_eh_enabled {
                                // Native EH: skip polling; fall through to next state.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else {
                                let Some(target_label) = op.value else {
                                    eprintln!(
                                        "WASM lowering warning: check_exception missing label in {} at op {}; falling through",
                                        func_ir.name, idx
                                    );
                                    let next_block = idx + 1;
                                    func.instruction(&Instruction::I64Const(next_block as i64));
                                    func.instruction(&Instruction::LocalSet(state_local));
                                    func.instruction(&Instruction::Br(depth));
                                    block_terminated = true;
                                    continue;
                                };
                                let Some(target_idx) = label_to_index.get(&target_label).copied()
                                else {
                                    eprintln!(
                                        "WASM lowering warning: unknown check_exception label {} in {} at op {}; falling through",
                                        target_label, func_ir.name, idx
                                    );
                                    let next_block = idx + 1;
                                    func.instruction(&Instruction::I64Const(next_block as i64));
                                    func.instruction(&Instruction::LocalSet(state_local));
                                    func.instruction(&Instruction::Br(depth));
                                    block_terminated = true;
                                    continue;
                                };
                                let target_block = target_idx;
                                let next_block = idx + 1;
                                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                                func.instruction(&Instruction::I64Const(0));
                                func.instruction(&Instruction::I64Ne);
                                func.instruction(&Instruction::If(BlockType::Empty));
                                func.instruction(&Instruction::I64Const(target_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::Else);
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::End);
                                block_terminated = true;
                            }
                        }
                        "ret" => {
                            let ret_local =
                                op.var.as_ref().and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                eprintln!(
                                    "WASM lowering warning: missing state-machine return local in {} op {} (var={:?}); returning None",
                                    func_ir.name, idx, op.var
                                );
                                func.instruction(&Instruction::I64Const(box_none()));
                            }
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "ret_void" => {
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        _ => {
                            emit_ops(
                                func,
                                std::slice::from_ref(op),
                                &mut scratch_control,
                                &mut scratch_try,
                                &mut label_stack,
                                &mut label_depths,
                                idx,
                            );
                        }
                    }
                    if block_terminated {
                        break;
                    }
                }

                let next_state = end;
                if !block_terminated {
                    func.instruction(&Instruction::I64Const(next_state as i64));
                    func.instruction(&Instruction::LocalSet(state_local));
                }
                func.instruction(&Instruction::Br(depth));

                if block_idx + 1 < block_count {
                    func.instruction(&Instruction::End);
                }
            }
            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else {
            let func = &mut func;
            let mut jump_labels: HashSet<i64> = HashSet::new();
            let mut label_order: Vec<i64> = Vec::new();
            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "jump" => {
                        if let Some(label_id) = op.value {
                            jump_labels.insert(label_id);
                        }
                    }
                    "label" => {
                        if let Some(label_id) = op.value {
                            label_order.push(label_id);
                        }
                    }
                    _ => {}
                }
            }
            let label_ids: Vec<i64> = label_order
                .into_iter()
                .filter(|label_id| jump_labels.contains(label_id))
                .collect();
            if !label_ids.is_empty() {
                for label_id in label_ids.iter().rev() {
                    func.instruction(&Instruction::Block(BlockType::Empty));
                    control_stack.push(ControlKind::Block);
                    label_depths.insert(*label_id, control_stack.len() - 1);
                    label_stack.push(*label_id);
                }
            }
            emit_ops(
                func,
                &func_ir.ops,
                &mut control_stack,
                &mut try_stack,
                &mut label_stack,
                &mut label_depths,
                0,
            );
            while !label_stack.is_empty() {
                label_stack.pop();
                func.instruction(&Instruction::End);
                control_stack.pop();
            }
            func.instruction(&Instruction::End);
        }

        // Accumulate tail call count from this function into the backend total.
        self.tail_calls_emitted += tail_call_count.get();

        self.codes.function(&func);
    }
}

fn encode_u32_leb128_padded(mut value: u32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn encode_i32_sleb128_padded(mut value: i32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn emit_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if func_index == u32::MAX {
        // Sentinel: this import was stripped in pure profile mode.
        // Trap if the code path is actually reached at runtime.
        func.instruction(&Instruction::Unreachable);
        return;
    }
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x10);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::Call(func_index));
    }
}

/// Emit a call or `unreachable` if the target import was stripped in pure profile mode.
#[allow(dead_code)]
fn emit_call_or_unreachable(
    func: &mut Function,
    reloc_enabled: bool,
    func_index: u32,
    skipped: &HashSet<u32>,
) {
    if skipped.contains(&func_index) {
        func.instruction(&Instruction::Unreachable);
    } else {
        emit_call(func, reloc_enabled, func_index);
    }
}

/// Emit a `return_call` instruction (WASM tail calls proposal).
/// The callee's return value becomes the caller's return value without growing the stack.
fn emit_return_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if func_index == u32::MAX {
        // Sentinel: this import was stripped in pure profile mode.
        func.instruction(&Instruction::Unreachable);
        return;
    }
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x12); // return_call opcode
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::ReturnCall(func_index));
    }
}

fn emit_call_indirect(func: &mut Function, reloc_enabled: bool, ty: u32, table: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(11);
        bytes.push(0x11);
        encode_u32_leb128_padded(ty, &mut bytes);
        encode_u32_leb128_padded(table, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::CallIndirect {
            type_index: ty,
            table_index: table,
        });
    }
}

fn emit_i32_const(func: &mut Function, reloc_enabled: bool, value: i32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x41);
        encode_i32_sleb128_padded(value, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::I32Const(value));
    }
}

fn emit_ref_func(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0xD2);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::RefFunc(func_index));
    }
}

fn emit_table_index_i32(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_i32_const(func, reloc_enabled, table_index as i32);
}

fn emit_table_index_i64(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_table_index_i32(func, reloc_enabled, table_index);
    func.instruction(&Instruction::I64ExtendI32U);
}

fn const_expr_i32_const_padded(value: i32) -> ConstExpr {
    let mut bytes = Vec::with_capacity(6);
    bytes.push(0x41);
    encode_i32_sleb128_padded(value, &mut bytes);
    ConstExpr::raw(bytes)
}

#[derive(Clone, Copy)]
enum PendingReloc {
    Function { offset: u32, func_index: u32 },
    Type { offset: u32, type_index: u32 },
    DataAddr { offset: u32, segment_index: u32 },
}

#[derive(Clone, Copy)]
struct RelocEntry {
    ty: u8,
    offset: u32,
    index: u32,
    addend: i32,
}

fn encode_reloc_section(
    name: &'static str,
    section_index: u32,
    entries: &[RelocEntry],
) -> CustomSection<'static> {
    let mut data = Vec::new();
    section_index.encode(&mut data);
    (entries.len() as u32).encode(&mut data);
    for entry in entries {
        data.push(entry.ty);
        entry.offset.encode(&mut data);
        entry.index.encode(&mut data);
        if matches!(entry.ty, 4 | 5) {
            entry.addend.encode(&mut data);
        }
    }
    CustomSection {
        name: name.into(),
        data: Cow::Owned(data),
    }
}

fn append_custom_section(bytes: &mut Vec<u8>, section: &impl Encode) {
    bytes.push(0);
    section.encode(bytes);
}

fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
) -> Vec<u8> {
    let mut func_imports: Vec<String> = Vec::new();
    let mut func_exports: HashMap<u32, String> = HashMap::new();
    let mut func_import_count = 0u32;
    let mut defined_func_count = 0u32;
    let mut table_import_count = 0u32;
    let mut table_defined_count = 0u32;
    let mut code_section_start = None;
    let mut code_section_index = None;
    let mut data_section_index = None;
    let mut element_section_index = None;
    let mut func_body_starts: Vec<usize> = Vec::new();
    let mut pending_code: Vec<PendingReloc> = Vec::new();
    let mut pending_data: Vec<PendingReloc> = Vec::new();
    let mut pending_elem: Vec<PendingReloc> = Vec::new();
    let mut section_index = 0u32;

    let mut parse_failed = false;
    for payload in Parser::new(0).parse_all(&bytes) {
        let payload = match payload {
            Ok(payload) => payload,
            Err(_) => {
                parse_failed = true;
                break;
            }
        };
        match payload {
            Payload::TypeSection(_) => {
                section_index += 1;
            }
            Payload::ImportSection(reader) => {
                section_index += 1;
                for import in reader.into_imports().flatten() {
                    match import.ty {
                        TypeRef::Func(_) => {
                            func_imports.push(import.name.to_string());
                            func_import_count += 1;
                        }
                        TypeRef::Table(_) => {
                            table_import_count += 1;
                        }
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                defined_func_count = reader.count();
                section_index += 1;
            }
            Payload::TableSection(reader) => {
                table_defined_count = reader.count();
                section_index += 1;
            }
            Payload::MemorySection(_) => {
                section_index += 1;
            }
            Payload::GlobalSection(_) => {
                section_index += 1;
            }
            Payload::ExportSection(reader) => {
                for export in reader.into_iter().flatten() {
                    if export.kind == ExternalKind::Func {
                        func_exports.insert(export.index, export.name.to_string());
                    }
                }
                section_index += 1;
            }
            Payload::StartSection { .. } => {
                section_index += 1;
            }
            Payload::ElementSection(reader) => {
                let element_section_start = reader.range().start;
                element_section_index = Some(section_index);
                section_index += 1;
                for element in reader.into_iter().flatten() {
                    if let ElementItems::Functions(funcs) = element.items {
                        for func in funcs.into_iter_with_offsets().flatten() {
                            let (pos, func_index) = func;
                            let offset = (pos.saturating_sub(element_section_start)) as u32;
                            pending_elem.push(PendingReloc::Function { offset, func_index });
                        }
                    }
                }
            }
            Payload::CodeSectionStart { range, .. } => {
                code_section_start = Some(range.start);
                code_section_index = Some(section_index);
                section_index += 1;
            }
            Payload::CodeSectionEntry(body) => {
                func_body_starts.push(body.range().start);
                if let Ok(mut ops) = body.get_operators_reader() {
                    while let Ok((op, op_offset)) = ops.read_with_offset() {
                        let start = match code_section_start {
                            Some(start) => start,
                            None => break,
                        };
                        match op {
                            Operator::Call { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            Operator::CallIndirect { type_index, .. } => {
                                let type_offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Type {
                                    offset: type_offset,
                                    type_index,
                                });
                            }
                            Operator::RefFunc { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Payload::DataSection(reader) => {
                let data_section_start = reader.range().start;
                data_section_index = Some(section_index);
                section_index += 1;
                for (segment_index, data) in reader.into_iter().enumerate() {
                    if let Ok(data) = data
                        && let DataKind::Active { offset_expr, .. } = data.kind
                    {
                        let mut ops = offset_expr.get_operators_reader();
                        if let Ok((Operator::I32Const { .. }, op_offset)) = ops.read_with_offset() {
                            let offset = (op_offset + 1).saturating_sub(data_section_start) as u32;
                            pending_data.push(PendingReloc::DataAddr {
                                offset,
                                segment_index: segment_index as u32,
                            });
                        }
                    }
                }
            }
            Payload::DataCountSection { .. } => {
                section_index += 1;
            }
            _ => {}
        }
    }
    if parse_failed {
        return bytes;
    }

    let code_section_start = match code_section_start {
        Some(start) => start,
        None => return bytes,
    };
    let code_section_index = match code_section_index {
        Some(index) => index,
        None => return bytes,
    };
    let data_section_index = data_section_index;

    for site in data_relocs {
        let def_index = site.func_index.saturating_sub(func_import_count) as usize;
        if let Some(body_start) = func_body_starts.get(def_index) {
            let offset = (body_start.saturating_sub(code_section_start) as u32)
                .saturating_add(site.offset_in_func);
            pending_code.push(PendingReloc::DataAddr {
                offset,
                segment_index: site.segment_index,
            });
        }
    }

    let total_funcs = func_import_count + defined_func_count;
    let mut func_symbol_map = vec![0u32; total_funcs as usize];
    let mut data_symbol_map = vec![0u32; data_segments.len()];
    let mut symbol_index = 0u32;

    let mut sym_tab = SymbolTable::new();
    let mut import_names: Vec<String> = Vec::new();
    for (idx, name) in func_imports.iter().enumerate() {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_EXPLICIT_NAME;
        let symbol_name = format!("molt_{name}");
        import_names.push(symbol_name);
        let name_ref = import_names.last().unwrap();
        sym_tab.function(flags, idx as u32, Some(name_ref));
        func_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }
    let mut func_names: Vec<String> = Vec::new();
    for def_idx in 0..defined_func_count {
        let func_index = func_import_count + def_idx;
        let export_name = func_exports.get(&func_index).cloned();
        // Keep linker symbol names module-scoped so linked output/runtime objects
        // cannot accidentally alias local function symbols with identical names.
        // Preserve explicit call_indirect export symbols because wasm_link.py
        // resolves/aliases those by name for runtime ABI wiring.
        let name = match export_name.as_deref() {
            Some("molt_main") | Some("molt_table_init") => export_name.clone().unwrap_or_default(),
            Some(exported) if exported.starts_with("molt_call_indirect") => exported.to_string(),
            Some(_) => format!("__molt_output_export_{func_index}"),
            None => format!("__molt_output_fn_{func_index}"),
        };
        func_names.push(name);
        let name_ref = func_names.last().unwrap();
        let flags = if export_name.is_some() {
            SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP
        } else {
            0
        };
        sym_tab.function(flags, func_index, Some(name_ref));
        func_symbol_map[func_index as usize] = symbol_index;
        symbol_index += 1;
    }

    for table_idx in 0..table_import_count {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_NO_STRIP;
        sym_tab.table(flags, table_idx, None);
        symbol_index += 1;
    }
    let mut table_names: Vec<String> = Vec::new();
    for table_idx in 0..table_defined_count {
        let index = table_import_count + table_idx;
        let name = format!("__molt_output_table_{index}");
        table_names.push(name);
        let name_ref = table_names.last().unwrap();
        sym_tab.table(0, index, Some(name_ref));
        symbol_index += 1;
    }

    let mut data_names: Vec<String> = Vec::new();
    for (idx, info) in data_segments.iter().enumerate() {
        let name = format!("__molt_output_data_{idx}");
        data_names.push(name);
        let name_ref = data_names.last().unwrap();
        sym_tab.data(
            0,
            name_ref,
            Some(DataSymbolDefinition {
                index: idx as u32,
                offset: 0,
                size: info.size,
            }),
        );
        data_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }

    let mut code_entries: Vec<RelocEntry> = Vec::new();
    let mut data_entries: Vec<RelocEntry> = Vec::new();
    let mut elem_entries: Vec<RelocEntry> = Vec::new();
    for reloc in pending_code {
        match reloc {
            PendingReloc::Function { offset, func_index } => {
                if let Some(index) = func_symbol_map.get(func_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 0,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
            PendingReloc::Type { offset, type_index } => {
                code_entries.push(RelocEntry {
                    ty: 6,
                    offset,
                    index: type_index,
                    addend: 0,
                });
            }
            PendingReloc::DataAddr {
                offset,
                segment_index,
            } => {
                if let Some(index) = data_symbol_map.get(segment_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 4,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
        }
    }

    for reloc in pending_data {
        if let PendingReloc::DataAddr {
            offset,
            segment_index,
        } = reloc
            && let Some(index) = data_symbol_map.get(segment_index as usize)
        {
            data_entries.push(RelocEntry {
                ty: 4,
                offset,
                index: *index,
                addend: 0,
            });
        }
    }

    for reloc in pending_elem {
        if let PendingReloc::Function { offset, func_index } = reloc
            && let Some(index) = func_symbol_map.get(func_index as usize)
        {
            elem_entries.push(RelocEntry {
                ty: 0,
                offset,
                index: *index,
                addend: 0,
            });
        }
    }

    code_entries.sort_by_key(|entry| entry.offset);
    data_entries.sort_by_key(|entry| entry.offset);
    elem_entries.sort_by_key(|entry| entry.offset);

    let mut linking = LinkingSection::new();
    linking.symbol_table(&sym_tab);
    append_custom_section(&mut bytes, &linking);
    if !code_entries.is_empty() {
        let reloc_code = encode_reloc_section("reloc.CODE", code_section_index, &code_entries);
        append_custom_section(&mut bytes, &reloc_code);
    }
    if !data_entries.is_empty()
        && let Some(index) = data_section_index
    {
        let reloc_data = encode_reloc_section("reloc.DATA", index, &data_entries);
        append_custom_section(&mut bytes, &reloc_data);
    }
    if !elem_entries.is_empty()
        && let Some(index) = element_section_index
    {
        let reloc_elem = encode_reloc_section("reloc.ELEM", index, &elem_entries);
        append_custom_section(&mut bytes, &reloc_elem);
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ---------------------------------------------------------------
    // br_table state dispatch
    // ---------------------------------------------------------------

    #[test]
    fn br_table_viable_for_dense_entries() {
        // 6 entries mapping states 0..=5 (dense, above threshold)
        let entries: Vec<(i64, i64)> = (0..6).map(|i| (i as i64, i as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "dense 6-entry range should be viable");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 0);
        assert_eq!(table_size, 6);
    }

    #[test]
    fn br_table_viable_with_offset_range() {
        // 5 entries starting at state 10: 10,11,12,13,14
        let entries: Vec<(i64, i64)> = (10..15).map(|i| (i as i64, (i - 10) as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "dense 5-entry range should be viable");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 10);
        assert_eq!(table_size, 5);
    }

    #[test]
    fn br_table_rejected_for_few_entries() {
        // Only 4 entries -- below BR_TABLE_MIN_ENTRIES (5)
        let entries: Vec<(i64, i64)> = (0..4).map(|i| (i as i64, i as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_none(), "4 entries should be below the threshold");
    }

    #[test]
    fn br_table_rejected_for_sparse_entries() {
        // 5 entries spanning 0..=100: table_size = 101, sparsity = 101/5 = 20.2 (> 8)
        let entries: Vec<(i64, i64)> = vec![(0, 0), (25, 1), (50, 2), (75, 3), (100, 4)];
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_none(), "sparsity 20 exceeds max allowed 8");
    }

    #[test]
    fn br_table_boundary_at_exactly_threshold() {
        // Exactly 5 entries -- the minimum required
        let entries: Vec<(i64, i64)> = (0..5).map(|i| (i as i64, i as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "exactly 5 entries should pass");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 0);
        assert_eq!(table_size, 5);
    }

    #[test]
    fn br_table_sparsity_at_max_boundary() {
        // 5 entries, table_size = 5 * 8 = 40 (exactly at sparsity limit)
        // entries: 0, 10, 20, 30, 39  ->  table_size = 40, sparsity = 40/5 = 8
        let entries: Vec<(i64, i64)> = vec![(0, 0), (10, 1), (20, 2), (30, 3), (39, 4)];
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "sparsity exactly 8 should be accepted");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 0);
        assert_eq!(table_size, 40);
    }

    #[test]
    fn br_table_sparsity_just_over_max() {
        // 5 entries, table_size = 41: sparsity = 41/5 = 8.2 (> 8)
        let entries: Vec<(i64, i64)> = vec![(0, 0), (10, 1), (20, 2), (30, 3), (40, 4)];
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_none(), "sparsity 8.2 should be rejected");
    }

    // ---------------------------------------------------------------
    // Dead local elimination -- read-variable scanning
    // ---------------------------------------------------------------

    /// Build a minimal OpIR with only the fields relevant to read-var scanning.
    fn make_op(kind: &str, args: Option<Vec<&str>>, var: Option<&str>, out: Option<&str>) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: args.map(|a| a.into_iter().map(String::from).collect()),
            var: var.map(String::from),
            out: out.map(String::from),
            ..Default::default()
        }
    }

    /// Replicate the read-var scanning logic from the compiler to test it in isolation.
    fn collect_read_vars(ops: &[OpIR]) -> HashSet<String> {
        let mut s = HashSet::new();
        for op in ops {
            if let Some(args) = &op.args {
                for arg in args {
                    s.insert(arg.clone());
                }
            }
            if let Some(var) = &op.var {
                s.insert(var.clone());
            }
        }
        s
    }

    #[test]
    fn read_vars_includes_args_and_var() {
        let ops = vec![
            make_op("add", Some(vec!["a", "b"]), None, Some("c")),
            make_op("load", None, Some("d"), Some("e")),
        ];
        let read_vars = collect_read_vars(&ops);
        assert!(read_vars.contains("a"), "arg 'a' should be in read set");
        assert!(read_vars.contains("b"), "arg 'b' should be in read set");
        assert!(read_vars.contains("d"), "var 'd' should be in read set");
        // 'c' and 'e' are outputs only -- they should NOT be in read_vars
        assert!(
            !read_vars.contains("c"),
            "output-only 'c' should NOT be in read set"
        );
        assert!(
            !read_vars.contains("e"),
            "output-only 'e' should NOT be in read set"
        );
    }

    #[test]
    fn read_vars_output_becomes_live_when_later_read() {
        let ops = vec![
            make_op("const", None, None, Some("x")),
            make_op("add", Some(vec!["x", "y"]), None, Some("z")),
        ];
        let read_vars = collect_read_vars(&ops);
        // 'x' is an output of const but also an arg of add -- should be live
        assert!(
            read_vars.contains("x"),
            "'x' should be live since it's read by add"
        );
        assert!(read_vars.contains("y"), "'y' should be live");
        // 'z' is output-only
        assert!(
            !read_vars.contains("z"),
            "'z' is output-only, should be dead"
        );
    }

    #[test]
    fn dead_local_all_outputs_dead() {
        // No op reads any variable -- all outputs are dead
        let ops = vec![
            make_op("const", None, None, Some("a")),
            make_op("const", None, None, Some("b")),
            make_op("const", None, None, Some("c")),
        ];
        let read_vars = collect_read_vars(&ops);
        assert!(read_vars.is_empty(), "no variable is ever read");
    }
}
