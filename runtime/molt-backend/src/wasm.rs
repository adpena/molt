use crate::{FunctionIR, OpIR, SimpleIR, TrampolineKind, TrampolineSpec};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
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
const GEN_CONTROL_SIZE: i32 = 48;
const TASK_KIND_FUTURE: i64 = 0;
const TASK_KIND_GENERATOR: i64 = 1;
const TASK_KIND_COROUTINE: i64 = 2;
const RELOC_TABLE_BASE_DEFAULT: u32 = 4096;

/// Poll/async function names that occupy the prefix slots of the indirect
/// function table (right after the sentinel slot at index 0).  Defined once
/// so the wrapper-generation loop, the index-lookup block, and the table
/// element initialisation all stay in sync automatically.
const POLL_TABLE_FUNCS: &[&str] = &[
    "async_sleep",
    "anext_default_poll",
    "asyncgen_poll",
    "promise_poll",
    "io_wait",
    "thread_poll",
    "process_poll",
    "ws_wait",
    "asyncio_wait_for_poll",
    "asyncio_wait_poll",
    "asyncio_gather_poll",
    "asyncio_socket_reader_read_poll",
    "asyncio_socket_reader_readline_poll",
    "asyncio_stream_reader_read_poll",
    "asyncio_stream_reader_readline_poll",
    "asyncio_stream_send_all_poll",
    "asyncio_sock_recv_poll",
    "asyncio_sock_connect_poll",
    "asyncio_sock_accept_poll",
    "asyncio_sock_recv_into_poll",
    "asyncio_sock_sendall_poll",
    "asyncio_sock_recvfrom_poll",
    "asyncio_sock_recvfrom_into_poll",
    "asyncio_sock_sendto_poll",
    "asyncio_timer_handle_poll",
    "asyncio_fd_watcher_poll",
    "asyncio_server_accept_loop_poll",
    "asyncio_ready_runner_poll",
    "contextlib_asyncgen_enter_poll",
    "contextlib_asyncgen_exit_poll",
    "contextlib_async_exitstack_exit_poll",
    "contextlib_async_exitstack_enter_context_poll",
];

fn prepare_lir_wasm_fast_output(
    tir_func: &crate::tir::function::TirFunction,
) -> Option<crate::tir::lower_to_wasm::WasmFunctionOutput> {
    let output = crate::tir::lower_to_wasm::lower_tir_to_wasm_boxed_i64_abi(tir_func)?;
    let has_placeholder_call = output
        .instructions
        .iter()
        .any(|inst| matches!(inst, Instruction::Call(0)));
    if has_placeholder_call {
        None
    } else {
        Some(output)
    }
}

fn is_production_lir_wasm_fast_path_name(func_name: &str) -> bool {
    func_name.contains("____molt_globals_builtin__")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReservedRuntimeCallableSpec {
    index: u32,
    runtime_name: &'static str,
    import_name: &'static str,
    arity: usize,
}

const RESERVED_RUNTIME_CALLABLE_SPECS: &[ReservedRuntimeCallableSpec] = &{
    macro_rules! entry_list {
        ($(($idx:expr, $sym:ident, $import:literal, $arity:expr))+) => {
            [
                $(
                    ReservedRuntimeCallableSpec {
                        index: $idx,
                        runtime_name: stringify!($sym),
                        import_name: $import,
                        arity: $arity,
                    },
                )+
            ]
        };
    }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wasm_runtime_callables.inc"
    ))
};
const RESERVED_RUNTIME_CALLABLE_COUNT: u32 = RESERVED_RUNTIME_CALLABLE_SPECS.len() as u32;
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
// Enabled by default; set MOLT_WASM_NATIVE_EH=0 to disable.
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

/// First dynamic type index; must equal the count of all statically-defined types.
///
/// Static signatures currently occupy indices 0..=40 inclusive. Dynamic user
/// arity signatures and wrapper signatures must start after that fixed set.
const STATIC_TYPE_COUNT: u32 = 41;

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
/// `Index<&str>` access inserts the key into a shared `BTreeSet` so we can
/// compute the set of *unused* imports after compilation finishes.
///
/// The `used` set is behind `Rc<RefCell<…>>` so that clones (needed to
/// work around borrow-checker constraints during `compile_func`) share
/// the same tracking set as the original.
#[derive(Clone)]
struct TrackedImportIds {
    inner: BTreeMap<String, u32>,
    used: Rc<RefCell<BTreeSet<String>>>,
}

impl TrackedImportIds {
    fn new(inner: BTreeMap<String, u32>) -> Self {
        Self {
            inner,
            used: Rc::new(RefCell::new(BTreeSet::new())),
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
    func_map: &'a BTreeMap<String, u32>,
    func_indices: &'a BTreeMap<String, u32>,
    trampoline_map: &'a BTreeMap<String, u32>,
    table_base: u32,
    import_ids: &'a TrackedImportIds,
    reloc_enabled: bool,
    /// Functions eligible for multi-value return optimization.
    /// Maps function name -> number of return values (2 or 3).
    multi_return_candidates: &'a BTreeMap<String, usize>,
    /// Functions whose WASM signature includes a leading closure (i64) parameter.
    /// The `call_guarded` fast path must extract closure bits from the callee
    /// object and prepend them to the argument list when calling these targets.
    closure_functions: &'a BTreeSet<String>,
    /// Functions that escape through function-object creation ops and therefore
    /// must preserve callable-object dispatch semantics when invoked via
    /// `call_guarded`.
    escaped_callable_targets: &'a BTreeSet<String>,
    /// Linear-memory offset of a scratch buffer used to spill `call_func` args.
    call_func_spill_offset: u32,
    /// Linear-memory offset of a shared scratch buffer used for outlined class_def
    /// payloads (bases followed by attribute key/value pairs).
    class_def_spill_offset: u32,
    /// Data segment ref for the 8-byte scratch slot used by `const_str` ops.
    const_str_scratch_segment: DataSegmentRef,
    /// Precomputed production-safe LIR-based wasm outputs keyed by function name.
    lir_fast_outputs: &'a BTreeMap<String, crate::tir::lower_to_wasm::WasmFunctionOutput>,
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

// Constant folding pass is now shared via crate::fold_constants in passes.rs.

fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
}

fn box_float(val: f64) -> i64 {
    if val.is_nan() {
        // Canonicalize NaN to avoid collision with the QNAN tag prefix.
        // Must match CANONICAL_NAN_BITS in molt-obj-model.
        0x7ff0_0000_0000_0001_u64 as i64
    } else {
        val.to_bits() as i64
    }
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

/// Emit WASM instructions to convert an f64 on the stack to a NaN-canonicalized i64.
/// Uses `scratch_local` (an i64 local) as temporary storage.
/// Expects: stack = [..., f64_val]
/// Produces: stack = [..., i64_boxed] where NaN is replaced with CANONICAL_NAN_BITS.
fn emit_f64_to_i64_canonical(func: &mut wasm_encoder::Function, scratch_local: u32) {
    // Reinterpret f64 to i64 raw bits, save in scratch
    func.instruction(&Instruction::I64ReinterpretF64);
    func.instruction(&Instruction::LocalTee(scratch_local));
    // Check if raw bits have QNAN prefix: (raw & QNAN) == QNAN
    func.instruction(&Instruction::I64Const(QNAN as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const(QNAN as i64));
    func.instruction(&Instruction::I64Eq);
    // select(canonical, raw, is_nan) — if is_nan is true (nonzero), picks canonical
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&Instruction::I64Const(CANONICAL_NAN_BITS as i64));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(scratch_local));
    func.instruction(&Instruction::End);
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

/// Cache of WASM local indices holding frequently-used i64 constants.
/// When a function body contains 3+ fast_int operations, these locals are
/// pre-allocated and initialized once at function entry, replacing repeated
/// `i64.const` immediates with cheaper `local.get` instructions.
#[derive(Clone, Copy, Default)]
struct ConstantCache {
    int_shift: Option<u32>,
    int_min: Option<u32>,
    int_max: Option<u32>,
    none_bits: Option<u32>,
    qnan_tag_mask: Option<u32>,
    qnan_tag_ptr: Option<u32>,
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
        if let Some(local) = self.none_bits {
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.qnan_tag_mask {
            func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.qnan_tag_ptr {
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
            func.instruction(&Instruction::LocalSet(local));
        }
    }

    /// Emit `box_none()` — uses cached local if available, otherwise literal.
    #[inline]
    fn emit_none(&self, func: &mut Function) {
        if let Some(local) = self.none_bits {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(box_none()));
        }
    }

    /// Emit `QNAN_TAG_MASK_I64` — uses cached local if available, otherwise literal.
    #[inline]
    fn emit_qnan_tag_mask(&self, func: &mut Function) {
        if let Some(local) = self.qnan_tag_mask {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
        }
    }

    /// Emit `QNAN_TAG_PTR_I64` — uses cached local if available, otherwise literal.
    #[inline]
    fn emit_qnan_tag_ptr(&self, func: &mut Function) {
        if let Some(local) = self.qnan_tag_ptr {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
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
    known_raw: &BTreeMap<u32, i64>,
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
    known_raw: &BTreeMap<u32, i64>,
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
fn emit_box_int_from_local_opt(
    func: &mut Function,
    src_local: u32,
    known_raw: &BTreeMap<u32, i64>,
) {
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

fn has_non_linear_control_flow(ops: &[OpIR]) -> bool {
    ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "if" | "else"
                | "end_if"
                | "loop_start"
                | "loop_index_start"
                | "loop_break_if_true"
                | "loop_break_if_false"
                | "loop_break"
                | "loop_continue"
                | "loop_end"
                | "for_iter_start"
                | "for_iter_end"
                | "while_start"
                | "while_end"
                | "try_start"
                | "try_end"
                | "async_for_start"
                | "async_for_end"
                | "jump"
                | "br_if"
                | "label"
                | "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
                | "state_label"
                | "check_exception"
                | "ret"
                | "ret_void"
        )
    })
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
    label_to_index: BTreeMap<i64, usize>,
    else_for_if: BTreeMap<usize, usize>,
    end_for_if: BTreeMap<usize, usize>,
    end_for_else: BTreeMap<usize, usize>,
    loop_continue_target: BTreeMap<usize, usize>,
    loop_break_target: BTreeMap<usize, usize>,
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

fn build_state_resume_maps(ops: &[OpIR]) -> (BTreeMap<i64, usize>, BTreeMap<String, i64>) {
    let mut state_map: BTreeMap<i64, usize> = BTreeMap::new();
    state_map.insert(0, 0);
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();

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

fn build_dense_state_remap_table(state_map: &BTreeMap<i64, usize>) -> Option<Vec<u8>> {
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

fn build_sparse_state_remap_entries(state_map: &BTreeMap<i64, usize>) -> Vec<(i64, i64)> {
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
    let target_to_case: BTreeMap<i64, usize> = unique_targets
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

/// WASM profile for import stripping (see docs/architecture/wasm-import-stripping.md §3A).
/// `Full` registers all host imports; `Pure` omits IO, ASYNC, and TIME categories
/// so the resulting module only depends on core runtime + arithmetic + collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmProfile {
    Full,
    Pure,
    /// Scan IR to include only imports that are actually used (plus a generous core set).
    Auto,
}

#[derive(Debug, Clone, Copy)]
pub struct WasmCompileOptions {
    pub reloc_enabled: bool,
    pub data_base: u32,
    pub table_base: u32,
    /// Enable native WASM exception handling (WASM 3.0 EH proposal).
    /// Enabled by default; set `MOLT_WASM_NATIVE_EH=0` to disable.
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
                    // Default: 64 MiB.  The split-runtime layout shares
                    // linear memory between the Rust runtime WASM module
                    // (whose data segments start at ~1 MiB and whose
                    // dlmalloc heap grows upward from there) and the
                    // output module.  A 1 MiB default would collide with
                    // the runtime's data region and cause string-pointer
                    // corruption on large module graphs.  64 MiB leaves
                    // ample headroom for the runtime heap.
                    .unwrap_or(64 * 1024 * 1024);
                let aligned = (raw + 7) & !7;
                aligned.min(u64::from(u32::MAX)) as u32
            },
            table_base: match std::env::var("MOLT_WASM_TABLE_BASE") {
                Ok(value) => value.parse::<u32>().unwrap_or(RELOC_TABLE_BASE_DEFAULT),
                Err(_) => RELOC_TABLE_BASE_DEFAULT,
            },
            native_eh_enabled: !matches!(std::env::var("MOLT_WASM_NATIVE_EH").as_deref(), Ok("0")),
            wasm_profile: match std::env::var("MOLT_WASM_PROFILE").as_deref() {
                Ok("pure") => WasmProfile::Pure,
                Ok("full") => WasmProfile::Full,
                _ => WasmProfile::Auto,
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
    // Dedup cache: maps byte content to existing segment ref.
    // HashMap is fine here — this map is only used for point lookups, never iterated.
    data_segment_cache: HashMap<Vec<u8>, DataSegmentRef>,
    molt_main_index: Option<u32>,
    options: WasmCompileOptions,
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
            data_segment_cache: HashMap::new(),
            molt_main_index: None,
            options,
            tail_calls_emitted: 0,
        }
    }

    fn add_data_segment(&mut self, reloc_enabled: bool, bytes: &[u8]) -> DataSegmentRef {
        self.add_data_segment_inner(reloc_enabled, bytes, true)
    }

    /// Like [`add_data_segment`] but skips the dedup cache.  Use this for
    /// segments that are **written to at runtime** (e.g. the call-func spill
    /// buffer) — caching them would allow a read-only segment with identical
    /// content to alias the mutable region, corrupting data when the spill
    /// buffer is written.
    fn add_data_segment_mutable(&mut self, reloc_enabled: bool, bytes: &[u8]) -> DataSegmentRef {
        self.add_data_segment_inner(reloc_enabled, bytes, false)
    }

    fn add_data_segment_inner(
        &mut self,
        reloc_enabled: bool,
        bytes: &[u8],
        cacheable: bool,
    ) -> DataSegmentRef {
        // Skip empty data segments entirely — they waste a segment header
        // (~7 bytes) in the binary for zero payload.
        if bytes.is_empty() {
            return DataSegmentRef {
                offset: self.data_offset,
                index: self.data_segments.len().saturating_sub(1) as u32,
            };
        }
        if cacheable {
            if let Some(existing) = self.data_segment_cache.get(bytes) {
                return *existing;
            }
        }
        let offset = self.data_offset;
        let byte_len: u32 = bytes
            .len()
            .try_into()
            .expect("data segment too large for WASM (>4 GiB)");
        let index = self.data_segments.len() as u32;
        let const_expr = if reloc_enabled {
            const_expr_i32_const_padded(offset as i32)
        } else {
            ConstExpr::i32_const(offset as i32)
        };
        self.data.active(0, &const_expr, bytes.iter().copied());
        // Checked arithmetic: detect overflow instead of silently wrapping,
        // which would place subsequent segments at wrong offsets and corrupt
        // data in shared linear memory.
        //
        // Size optimization: use 4-byte alignment for small segments (<=4 bytes)
        // instead of always 8-byte aligning.  This saves 4 bytes of padding per
        // small segment (e.g. single i32 constants, short string literals).
        // Larger segments still get 8-byte alignment for i64 load/store perf.
        let align_mask: u32 = if byte_len <= 4 { 3 } else { 7 };
        self.data_offset = offset
            .checked_add(byte_len)
            .and_then(|v| v.checked_add(align_mask))
            .map(|v| v & !align_mask)
            .expect("WASM data segment offset overflow (>4 GiB total data)");
        let info = DataSegmentInfo { size: byte_len };
        self.data_segments.push(info);
        let data_ref = DataSegmentRef { offset, index };
        if cacheable {
            self.data_segment_cache.insert(bytes.to_vec(), data_ref);
        }
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

    /// Like [`emit_data_ptr`] but pushes an **i32** value (no i64 extension).
    /// Useful when the address is consumed by an instruction that expects i32,
    /// e.g. `string_from_bytes`'s `out` parameter or `I64Load`'s address.
    fn emit_data_ptr_i32(
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
    fn detect_multi_return_candidates(ir: &SimpleIR) -> BTreeMap<String, usize> {
        // callee -> Option<arity>  (None means conflicting arities => ineligible)
        let mut candidate_arity: BTreeMap<String, Option<usize>> = BTreeMap::new();

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
                let mut seen_indices: BTreeSet<i64> = BTreeSet::new();
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
                    std::collections::btree_map::Entry::Vacant(e) => {
                        e.insert(Some(unpack_count));
                    }
                    std::collections::btree_map::Entry::Occupied(mut e) => {
                        if *e.get() != Some(unpack_count) {
                            // Conflicting arities across call sites — not eligible.
                            *e.get_mut() = None;
                        }
                    }
                }
            }
        }

        let call_site_candidates: BTreeMap<String, usize> = candidate_arity
            .into_iter()
            .filter_map(|(name, arity)| arity.map(|a| (name, a)))
            .collect();

        // Phase 2: Verify the callee function body — every `ret` must return
        // a variable that was produced by a `tuple_new` with the expected arity.
        // This ensures the callee genuinely always returns a fixed-size tuple.
        let func_map: BTreeMap<&str, &FunctionIR> =
            ir.functions.iter().map(|f| (f.name.as_str(), f)).collect();

        call_site_candidates
            .into_iter()
            .filter(|(name, expected_arity)| {
                let Some(func_ir) = func_map.get(name.as_str()) else {
                    return false;
                };
                // Track which variables are produced by tuple_new of the right arity.
                let mut tuple_new_vars: BTreeSet<String> = BTreeSet::new();
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

    /// Scan the IR to determine which host imports are actually referenced.
    /// Uses OP_IMPORT_DEPS table so the codegen declares its own dependencies.
    /// Returns a BTreeSet of import names. Used by `WasmProfile::Auto` to
    /// skip unreferenced imports (sentinel u32::MAX).
    fn collect_required_imports(ir: &SimpleIR) -> BTreeSet<String> {
        use crate::wasm_imports::OP_IMPORT_DEPS;

        let mut required: BTreeSet<String> = BTreeSet::new();

        // Build a lookup from the deps table.
        let deps_map: BTreeMap<&str, &[&str]> =
            OP_IMPORT_DEPS.iter().map(|&(k, v)| (k, v)).collect();

        // Structural imports: always needed regardless of IR content.
        if let Some(structural) = deps_map.get("__structural__") {
            for &name in *structural {
                required.insert(name.to_string());
            }
        }

        // Build the set of all known import names for auto-discovery.
        let known_imports: BTreeSet<&str> = crate::wasm_imports::IMPORT_REGISTRY
            .iter()
            .map(|&(name, _)| name)
            .collect();

        // Scan IR ops: each op declares its own import dependencies.
        for func_ir in &ir.functions {
            for op in &func_ir.ops {
                let kind = op.kind.as_str();

                // Check explicit dependency table first.
                if let Some(deps) = deps_map.get(kind) {
                    for &dep in *deps {
                        required.insert(dep.to_string());
                    }
                }
                // Auto-discovery: if op kind matches a known import name, include it.
                else if known_imports.contains(kind) {
                    required.insert(kind.to_string());
                }

                // builtin_func ops reference imports by s_value (with molt_ prefix).
                if kind == "builtin_func" {
                    if let Some(name) = op.s_value.as_ref() {
                        let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                        required.insert(import_name.to_string());
                    }
                }

                // Prefix-based discovery for stdlib groups.
                // If the op kind starts with a known stdlib prefix, include it.
                // Group expansions (e.g., ws_ → all websocket imports) are handled
                // by OP_IMPORT_DEPS entries above, not here.
                for prefix in [
                    "os_",
                    "path_",
                    "time_",
                    "struct_",
                    "importlib_",
                    "asyncio_",
                    "contextlib_async",
                    "socket_",
                    "file_",
                    "stream_",
                    "lock_",
                    "rlock_",
                    "thread_",
                    "process_",
                    "db_",
                    "ws_",
                    "cancel_token_",
                    "chan_",
                    "string_",
                    "bytes_",
                    "bytearray_",
                    "math_",
                    "json_",
                    "msgpack_",
                    "cbor_",
                    "vec_",
                    "heapq_",
                    "buffer2d_",
                    "statistics_",
                    "weakref_",
                    "memoryview_",
                    "taq_",
                    "sys_",
                    "dataclass_",
                ] {
                    if kind.starts_with(prefix) {
                        required.insert(kind.to_string());
                        break;
                    }
                }

                // Special singleton matches
                match kind {
                    "socketpair" | "cancelled" | "cancel_current" | "spawn" | "block_on"
                    | "sleep_register" | "intarray_from_seq" | "enumerate" | "aiter" | "anext"
                    | "open_builtin" | "compile_builtin" | "getargv" | "getpid" | "getframe"
                    | "getcwd" | "getrecursionlimit" | "setrecursionlimit" | "env_get"
                    | "env_snapshot" | "os_name" | "errno_constants" => {
                        required.insert(kind.to_string());
                    }
                    _ => {}
                }
            }

            // Scan poll functions for poll import references.
            if func_ir.name.ends_with("_poll") {
                for op in &func_ir.ops {
                    if (op.kind == "call_func" || op.kind == "invoke_ffi")
                        && let Some(s) = op.s_value.as_ref()
                        && s.ends_with("_poll")
                    {
                        let import_name = s.strip_prefix("molt_").unwrap_or(s.as_str());
                        required.insert(import_name.to_string());
                    }
                }
            }
        }

        required
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        let mut ir = ir;
        crate::apply_profile_order(&mut ir);
        for func_ir in &mut ir.functions {
            crate::rewrite_stateful_loops(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::elide_dead_struct_allocs(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::escape_analysis(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::rc_coalescing(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::fold_constants(&mut func_ir.ops);
            crate::passes::hoist_loop_invariants(func_ir);
        }
        let mut lir_fast_outputs: BTreeMap<String, crate::tir::lower_to_wasm::WasmFunctionOutput> =
            BTreeMap::new();
        // ── TIR optimization pipeline (default ON; set MOLT_TIR_OPT=0 to disable) ──
        if crate::env_setting("MOLT_TIR_OPT").as_deref() != Some("0") {
            let tir_dump = crate::env_setting("TIR_DUMP").as_deref() == Some("1");
            let tir_stats = crate::env_setting("TIR_OPT_STATS").as_deref() == Some("1");
            let mut tir_cache =
                crate::tir::cache::CompilationCache::open(std::path::PathBuf::from(".molt_cache"));
            for func_ir in &mut ir.functions {
                // Compute a stable content hash from the function name + input ops.
                let body_bytes = crate::tir::serialize::serialize_ops(&func_ir.ops);
                let content_hash = crate::tir::cache::CompilationCache::compute_hash_with_signature(
                    &func_ir.name,
                    &func_ir.params,
                    func_ir.param_types.as_deref(),
                    &body_bytes,
                );

                // Cache hit: restore previously optimized ops and skip the pipeline.
                if let Some(cached_bytes) = tir_cache.get(&content_hash) {
                    if let Some(cached_ops) = crate::tir::serialize::deserialize_ops(&cached_bytes)
                    {
                        func_ir.ops = cached_ops;
                        let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
                        crate::tir::type_refine::refine_types(&mut tir_func);
                        if is_production_lir_wasm_fast_path_name(&func_ir.name)
                            && let Some(output) = prepare_lir_wasm_fast_output(&tir_func)
                        {
                            lir_fast_outputs.insert(func_ir.name.clone(), output);
                        }
                        continue;
                    }
                }

                let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
                crate::tir::type_refine::refine_types(&mut tir_func);
                let stats = crate::tir::passes::run_pipeline(&mut tir_func);
                crate::tir::type_refine::refine_types(&mut tir_func);
                let type_map = crate::tir::type_refine::extract_type_map(&tir_func);
                if tir_dump {
                    eprintln!("{}", crate::tir::printer::print_function(&tir_func));
                }
                if tir_stats {
                    for s in &stats {
                        eprintln!(
                            "[TIR] {}: {} changed, {} removed, {} added",
                            s.name, s.values_changed, s.ops_removed, s.ops_added
                        );
                    }
                }
                if is_production_lir_wasm_fast_path_name(&func_ir.name)
                    && let Some(output) = prepare_lir_wasm_fast_output(&tir_func)
                {
                    lir_fast_outputs.insert(func_ir.name.clone(), output);
                }
                let optimized_ops =
                    crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func, &type_map);
                assert!(
                    crate::tir::lower_to_simple::validate_labels(&optimized_ops),
                    "TIR roundtrip emitted invalid labels for '{}' (WASM)",
                    func_ir.name
                );
                let serialized = crate::tir::serialize::serialize_ops(&optimized_ops);
                tir_cache.put(&content_hash, &serialized, vec![]);
                func_ir.ops = optimized_ops;
            }
            // Persist the updated cache index so future runs benefit.
            tir_cache.save_index();
        }

        crate::inline_functions(&mut ir);

        // Megafunction splitting: prevent O(n²) in wasm-encoder for huge functions.
        crate::split_megafunctions(&mut ir);

        // Dead function elimination: remove unreachable functions after inlining.
        crate::eliminate_dead_functions(&mut ir);

        if let Some(config) = crate::should_dump_ir() {
            for func_ir in &ir.functions {
                if crate::dump_ir_matches(&config, &func_ir.name) {
                    crate::dump_ir_ops(func_ir, &config.mode);
                }
            }
        }

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
        let mut escaped_callable_targets: BTreeSet<String> = BTreeSet::new();
        let mut task_kinds: BTreeMap<String, TrampolineKind> = BTreeMap::new();
        let mut task_closure_sizes: BTreeMap<String, i64> = BTreeMap::new();
        for func_ir in &ir.functions {
            let mut func_obj_names: BTreeMap<String, String> = BTreeMap::new();
            let mut const_values: BTreeMap<String, i64> = BTreeMap::new();
            let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();
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
                        escaped_callable_targets.insert(name.clone());
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
                    "builtin_func" => {
                        let Some(name) = op.s_value.as_ref() else {
                            continue;
                        };
                        escaped_callable_targets.insert(name.clone());
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
        let mut function_has_ret: BTreeMap<String, bool> = BTreeMap::new();
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
            function_has_ret.insert(
                func_ir.name.clone(),
                func_ir.ops.iter().any(|op| op.kind == "ret"),
            );
        }

        // Trampolines now handle multi-value return callees by reconstructing
        // a tuple from the N return values (see compile_trampoline), so we no
        // longer need to exclude trampolined functions from the optimization.
        let multi_return_candidates: BTreeMap<String, usize> =
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

        // Type 35: (i64 * 9) -> i64 (high-arity builtins)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 9),
            std::iter::once(ValType::I64),
        );
        // Type 36: (i64 * 10) -> i64 (json_dumps_ex)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 10),
            std::iter::once(ValType::I64),
        );
        // Type 37: (i64 * 11) -> i64 (reserved high-arity)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 11),
            std::iter::once(ValType::I64),
        );
        // Type 38: (i64 * 12) -> i64 (reserved high-arity)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 12),
            std::iter::once(ValType::I64),
        );
        // Type 39: (i32, i64) -> () (obj_set_state)
        self.types
            .function([ValType::I32, ValType::I64], std::iter::empty::<ValType>());
        // Type 40: (i64, i32, i64, i32, i64, i64, i64, i64) -> i64 (guarded_class_def)
        self.types.function(
            [
                ValType::I64,
                ValType::I32,
                ValType::I64,
                ValType::I32,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
            ],
            std::iter::once(ValType::I64),
        );

        // Build the set of import name prefixes to skip in "pure" profile mode.
        // In pure mode, IO/ASYNC/TIME imports are omitted entirely. Any code path
        // that references a skipped import will trigger a clear compile-time panic.
        let is_pure = self.options.wasm_profile == WasmProfile::Pure;
        let is_auto = self.options.wasm_profile == WasmProfile::Auto;

        // In Auto mode, scan the IR to determine which imports are actually used.
        let auto_required: Option<BTreeSet<String>> = if is_auto {
            let mut required = Self::collect_required_imports(&ir);
            // Linked/reloc wasm can materialize these constructor callables at
            // runtime via method caches even when no IR op mentions the imports
            // directly. If Auto prunes them here, wrapper emission degrades them
            // to sentinel traps and the linked table contract breaks.
            if self.options.reloc_enabled {
                required.extend(
                    RESERVED_RUNTIME_CALLABLE_SPECS
                        .iter()
                        .map(|spec| spec.import_name.to_string()),
                );
            }
            Some(required)
        } else {
            None
        };
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
            // In auto mode, skip imports not in the required set.
            if let Some(ref required) = auto_required {
                if !required.contains(name) {
                    ids.insert(name.to_string(), u32::MAX);
                    return;
                }
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
                9 => 35,
                10 => 36,
                11 => 37,
                12 => 38,
                _ => panic!("unsupported simple i64 import arity {arity}"),
            }
        };

        // Host Imports — driven by static registry (see wasm_imports.rs).
        for &(name, type_idx) in crate::wasm_imports::IMPORT_REGISTRY {
            add_import(name, type_idx, &mut self.import_ids);
        }

        let reloc_enabled = self.options.reloc_enabled;

        let mut max_func_arity = 0usize;
        let mut max_call_arity = 0usize;
        let mut max_class_def_words = 0usize;
        let mut builtin_trampoline_specs: BTreeMap<String, usize> = BTreeMap::new();
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
                if op.kind == "class_def"
                    && let Some(meta) = op.s_value.as_deref()
                {
                    let mut parts = meta.split(',');
                    let nbases = parts
                        .next()
                        .and_then(|s| s.parse::<usize>().ok())
                        .expect("class_def metadata missing base count");
                    let nattrs = parts
                        .next()
                        .and_then(|s| s.parse::<usize>().ok())
                        .expect("class_def metadata missing attr count");
                    let words = nbases.max(1) + (nattrs * 2).max(1);
                    max_class_def_words = max_class_def_words.max(words);
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
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            if !self.import_ids.contains_key(spec.import_name) {
                auto_import_names.push((spec.import_name.to_string(), spec.arity));
            }
        }
        auto_import_names.sort_by(|a, b| a.0.cmp(&b.0));
        auto_import_names.dedup_by(|a, b| a.0 == b.0);
        for (import_name, arity) in auto_import_names {
            add_import(
                import_name.as_str(),
                simple_i64_import_type(arity),
                &mut self.import_ids,
            );
        }
        self.func_count = import_idx;

        // Per-app intrinsic manifest: serialize used intrinsic names as a
        // NUL-separated data segment so the runtime only registers these.
        let manifest_bytes: Vec<u8> = {
            let mut buf = Vec::new();
            for (i, name) in builtin_trampoline_specs.keys().enumerate() {
                if i > 0 {
                    buf.push(0);
                }
                buf.extend_from_slice(name.as_bytes());
            }
            buf
        };
        let manifest_segment = self.add_data_segment(reloc_enabled, &manifest_bytes);
        let manifest_len = manifest_bytes.len();

        // Allocate a scratch buffer in linear memory for spilling call_func args.
        // Size: max(max_call_arity, 1) * 8 bytes (one i64 per arg).
        // SAFETY: This single-segment spill buffer is safe under reentrant calls
        // because `molt_call_func_dispatch` copies args into a Rust Vec<u64>
        // before dispatching, so nested WASM→runtime→WASM calls never observe
        // stale data in this buffer.
        let spill_slots = max_call_arity.max(1);
        let spill_bytes = vec![0u8; spill_slots * 8];
        let spill_segment = self.add_data_segment_mutable(reloc_enabled, &spill_bytes);
        let call_func_spill_offset = spill_segment.offset;

        // Shared outlined class_def spill buffer. The runtime helper snapshots the
        // bases/attrs payload before nested calls, so reentrant wasm->runtime->wasm
        // execution cannot observe stale scratch contents.
        let class_def_words = max_class_def_words.max(2);
        let class_def_bytes = vec![0u8; class_def_words * 8];
        let class_def_segment = self.add_data_segment_mutable(reloc_enabled, &class_def_bytes);
        let class_def_spill_offset = class_def_segment.offset;

        // Allocate an 8-byte scratch buffer in linear memory for const_str
        // operations.  Previously each const_str allocated a fresh 8-byte
        // heap object via `alloc(8)` to serve as the `out` parameter for
        // `string_from_bytes`, then leaked it (never dec-refed).  For large
        // modules with hundreds of string constants this wasted significant
        // heap space, bringing the heap closer to the output data region in
        // the split-runtime layout and contributing to heap-into-data
        // corruption.  Using a fixed scratch slot eliminates both the leak
        // and the per-string alloc call overhead.
        let const_str_scratch_bytes = vec![0u8; 8];
        let const_str_scratch_segment =
            self.add_data_segment_mutable(reloc_enabled, &const_str_scratch_bytes);

        let mut user_type_map: BTreeMap<usize, u32> = BTreeMap::new();
        // Types 0-38 are defined above (0-30 single-return, 31-34 multi-value, 35-38 high-arity);
        // start new dynamic signatures after them.
        let mut next_type_idx = STATIC_TYPE_COUNT;
        for func_ir in &ir.functions {
            if func_ir.name.ends_with("_poll") {
                continue;
            }
            let arity = func_ir.params.len();
            if let std::collections::btree_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
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
        let mut multi_return_type_map: BTreeMap<(usize, usize), u32> = BTreeMap::new();
        {
            // Collect unique (param_count, return_count) pairs from candidates.
            let func_param_counts: BTreeMap<&str, usize> = ir
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
            if let std::collections::btree_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        for arity in 0..=max_call_indirect {
            let sig_idx = *user_type_map.get(&(arity + 1)).unwrap_or_else(|| {
                panic!("missing call_indirect signature for arity {}", arity + 1)
            });
            let callee_idx = *user_type_map
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
        sentinel.instruction(&Instruction::Unreachable);
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
            ("molt_len_dict", "len_dict", 1),
            ("molt_len_list", "len_list", 1),
            ("molt_len_set", "len_set", 1),
            ("molt_len_str", "len_str", 1),
            ("molt_len_tuple", "len_tuple", 1),
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
        let reserved_runtime_callable_names: BTreeSet<&str> = RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .map(|spec| spec.runtime_name)
            .collect();
        let hardcoded_builtin_runtime_names: BTreeSet<&str> = builtin_table_funcs
            .iter()
            .map(|(runtime_name, _, _)| *runtime_name)
            .collect();
        let mut auto_builtin_table_funcs: Vec<(String, String, usize)> = builtin_trampoline_specs
            .iter()
            .filter(|(runtime_name, _)| {
                !hardcoded_builtin_runtime_names.contains(runtime_name.as_str())
                    && !reserved_runtime_callable_names.contains(runtime_name.as_str())
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
        let mut compact_builtin_trampoline_funcs: Vec<(String, usize)> = Vec::new();
        let builtin_runtime_names: BTreeSet<&str> = builtin_table_funcs
            .iter()
            .map(|(runtime_name, _, _)| *runtime_name)
            .chain(
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
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
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, _, _)| runtime_name.as_str()),
            )
        {
            if reserved_runtime_callable_names.contains(runtime_name) {
                continue;
            }
            if let Some(arity) = builtin_trampoline_specs.get(runtime_name) {
                compact_builtin_trampoline_funcs.push((runtime_name.to_string(), *arity));
            }
        }
        // Intrinsic ABIs are canonicalized to i64/u64 for dynamic function-object dispatch.
        // Keep wrapper conversion sets empty so generated wrappers preserve 64-bit bits values.
        let builtin_i32_arg0_imports: BTreeSet<&str> = [].into_iter().collect();
        let builtin_i32_return_imports: BTreeSet<&str> = [].into_iter().collect();
        let void_builtin_imports: BTreeSet<&str> = [
            "process_drop",
            "socket_drop",
            "stream_close",
            "stream_drop",
            "ws_close",
            "ws_drop",
        ]
        .into_iter()
        .collect();
        let mut builtin_wrapper_funcs: Vec<(String, String, usize)> =
            RESERVED_RUNTIME_CALLABLE_SPECS
                .iter()
                .map(|spec| {
                    (
                        spec.runtime_name.to_string(),
                        spec.import_name.to_string(),
                        spec.arity,
                    )
                })
                .collect();
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
            // Only generate wrappers for builtins that are actually referenced
            // by user code (present in builtin_trampoline_specs). With table
            // compaction, unreferenced builtins are omitted entirely — their
            // wrappers would be dead code wasting space in the code section.
            if builtin_trampoline_specs.contains_key(runtime_name.as_str()) {
                builtin_wrapper_funcs.push((runtime_name, import_name, arity));
            }
        }
        if builtin_trampoline_specs.len() != compact_builtin_trampoline_funcs.len() {
            for name in builtin_trampoline_specs.keys() {
                if !builtin_runtime_names.contains(name.as_str()) {
                    panic!("builtin {name} missing from wasm table");
                }
            }
        }
        let compact_builtin_table_len: usize = builtin_table_funcs
            .iter()
            .map(|(rn, _, _)| (*rn).to_string())
            .chain(auto_builtin_table_funcs.iter().map(|(rn, _, _)| rn.clone()))
            .filter(|rn| builtin_trampoline_specs.contains_key(rn.as_str()))
            .count();
        // Table compaction: only count referenced builtins for the table size.
        // Unreferenced builtins are omitted entirely (not sentinel-filled).
        let table_base: u32 = self.options.table_base;
        let split_runtime_runtime_table_min =
            std::env::var("MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN")
                .ok()
                .and_then(|raw| raw.parse::<u32>().ok());
        let split_runtime_owned_slot_start = split_runtime_runtime_table_min
            .map(|min| min.saturating_sub(table_base) as usize)
            .unwrap_or(0);
        // 1 sentinel slot + one slot per POLL_TABLE_FUNCS entry.
        // Derived dynamically so adding/removing poll functions cannot desync.
        let poll_table_prefix = (1 + POLL_TABLE_FUNCS.len()) as u32;
        let reserved_runtime_callable_table_len = RESERVED_RUNTIME_CALLABLE_COUNT as usize;
        let table_len = (poll_table_prefix as usize
            + reserved_runtime_callable_table_len * 2
            + compact_builtin_table_len
            + compact_builtin_trampoline_funcs.len()
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

        let mut builtin_wrapper_indices = BTreeMap::new();
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

        let mut table_import_wrappers = BTreeMap::new();
        if reloc_enabled {
            for import_name in POLL_TABLE_FUNCS {
                let arity = 1usize; // all poll functions take 1 arg
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

        // Build poll-function table prefix from POLL_TABLE_FUNCS.
        // Replace sentinel u32::MAX indices with sentinel_func_idx so the
        // element section only contains valid function indices.
        let safe_idx = |idx: u32| -> u32 {
            if idx == u32::MAX {
                sentinel_func_idx
            } else {
                idx
            }
        };
        let mut table_indices = vec![sentinel_func_idx]; // slot 0 = sentinel
        for &name in POLL_TABLE_FUNCS {
            let idx = *table_import_wrappers
                .get(name)
                .unwrap_or(&self.import_ids[name]);
            table_indices.push(safe_idx(idx));
        }
        debug_assert_eq!(table_indices.len(), poll_table_prefix as usize);
        let mut func_to_table_idx = BTreeMap::new();
        let mut func_to_index = BTreeMap::new();
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

        let reserved_runtime_callable_table_start = poll_table_prefix;
        let reserved_runtime_trampoline_table_start =
            reserved_runtime_callable_table_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let compact_builtin_table_start =
            reserved_runtime_trampoline_table_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let split_runtime_shared_abi_slot_end = compact_builtin_table_start as usize;
        let compact_builtin_trampoline_table_start =
            compact_builtin_table_start + compact_builtin_table_len as u32;
        let user_func_table_start =
            compact_builtin_trampoline_table_start + compact_builtin_trampoline_funcs.len() as u32;
        let user_trampoline_table_start = user_func_table_start + ir.functions.len() as u32;

        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            let wrapper_idx = *builtin_wrapper_indices
                .get(&runtime_name)
                .unwrap_or_else(|| panic!("reserved runtime wrapper missing for {runtime_name}"));
            func_to_table_idx.insert(
                runtime_name.clone(),
                reserved_runtime_callable_table_start + spec.index,
            );
            func_to_index.insert(runtime_name, wrapper_idx);
            table_indices.push(wrapper_idx);
        }

        let mut compact_builtin_entries: Vec<(String, u32)> = Vec::new();
        // Table compaction: only allocate slots for referenced builtins.
        // Unreferenced builtins are completely omitted from the element table.
        let mut compact_slot = 0u32;
        for (runtime_name, import_name, _) in builtin_table_funcs
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
            let runtime_key = runtime_name;
            let is_referenced = builtin_trampoline_specs.contains_key(runtime_key.as_str());
            if !is_referenced {
                continue; // Omit — no slot allocated.
            }
            let idx = compact_slot + compact_builtin_table_start;
            func_to_table_idx.insert(runtime_key.clone(), idx);
            let target_index = if let Some(wrapper_idx) = builtin_wrapper_indices.get(&runtime_key)
            {
                func_to_index.insert(runtime_key, *wrapper_idx);
                *wrapper_idx
            } else {
                let import_idx = self
                    .import_ids
                    .get(&import_name)
                    .copied()
                    .unwrap_or(sentinel_func_idx);
                // Replace sentinel u32::MAX with sentinel_func_idx for element section validity.
                let safe = if import_idx == u32::MAX {
                    sentinel_func_idx
                } else {
                    import_idx
                };
                func_to_index.insert(runtime_key, safe);
                safe
            };
            compact_builtin_entries.push((import_name, target_index));
            compact_slot += 1;
        }
        debug_assert_eq!(
            compact_slot as usize, compact_builtin_table_len,
            "compact slot count must match pre-computed builtin_table_len"
        );

        let user_func_start = self.func_count;
        let user_func_count = ir.functions.len() as u32;
        let builtin_trampoline_count =
            RESERVED_RUNTIME_CALLABLE_COUNT + compact_builtin_trampoline_funcs.len() as u32;
        let builtin_trampoline_start = user_func_start + user_func_count;
        let user_trampoline_start = builtin_trampoline_start + builtin_trampoline_count;
        let reserved_runtime_trampoline_func_start = builtin_trampoline_start;
        let compact_builtin_trampoline_func_start =
            reserved_runtime_trampoline_func_start + RESERVED_RUNTIME_CALLABLE_COUNT;

        let mut func_to_trampoline_idx = BTreeMap::new();
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            func_to_trampoline_idx.insert(
                runtime_name,
                reserved_runtime_trampoline_table_start + spec.index,
            );
            table_indices.push(reserved_runtime_trampoline_func_start + spec.index);
        }
        for (_import_name, target_index) in &compact_builtin_entries {
            table_indices.push(*target_index);
        }
        for (i, (name, _)) in compact_builtin_trampoline_funcs.iter().enumerate() {
            let idx = compact_builtin_trampoline_table_start + i as u32;
            func_to_trampoline_idx.insert(name.clone(), idx);
            table_indices.push(compact_builtin_trampoline_func_start + i as u32);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = user_func_table_start + i as u32;
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            func_to_index.insert(func_ir.name.clone(), user_func_start + i as u32);
            table_indices.push(user_func_start + i as u32);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = user_trampoline_table_start + i as u32;
            func_to_trampoline_idx.insert(func_ir.name.clone(), idx);
            table_indices.push(user_trampoline_start + i as u32);
        }

        let import_ids = self.import_ids.clone();

        // Build the set of functions whose WASM signature includes a leading
        // closure parameter.  The `call_guarded` fast path needs this to
        // extract the closure environment from the callee object and prepend
        // it when directly calling the target.
        let closure_functions: BTreeSet<String> = default_trampoline_spec
            .iter()
            .filter_map(|(name, &(_arity, has_closure))| {
                if has_closure {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        let compile_ctx = CompileFuncContext {
            func_map: &func_to_table_idx,
            func_indices: &func_to_index,
            trampoline_map: &func_to_trampoline_idx,
            import_ids: &import_ids,
            reloc_enabled,
            table_base,
            multi_return_candidates: &multi_return_candidates,
            closure_functions: &closure_functions,
            escaped_callable_targets: &escaped_callable_targets,
            call_func_spill_offset,
            class_def_spill_offset,
            const_str_scratch_segment,
            lir_fast_outputs: &lir_fast_outputs,
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
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let name = spec.runtime_name;
            let arity = spec.arity;
            let target_idx = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("reserved runtime trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx.get(name).unwrap_or_else(|| {
                panic!("reserved runtime trampoline table slot missing for {name}")
            });
            let table_idx = table_base + table_slot;
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity,
                    has_closure: false,
                    kind: TrampolineKind::Plain,
                    closure_size: 0,
                    target_has_ret: true,
                },
                None,
            );
        }
        if self.func_count != compact_builtin_trampoline_func_start {
            panic!(
                "wasm compact builtin trampoline index mismatch: expected {compact_builtin_trampoline_func_start}, got {}",
                self.func_count
            );
        }
        for (name, arity) in &compact_builtin_trampoline_funcs {
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
                    target_has_ret: true,
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
                    target_has_ret: *function_has_ret.get(target_name).unwrap_or(&true),
                },
                mr_count,
            );
        }

        let mut element_section = None;
        let mut element_payload = None;
        if reloc_enabled {
            let table_init_index = self.compile_table_init(
                reloc_enabled,
                table_base,
                &table_indices,
                split_runtime_owned_slot_start,
                split_runtime_shared_abi_slot_end,
            );
            self.exports
                .export("molt_table_init", ExportKind::Func, table_init_index);
            let main_index = self
                .molt_main_index
                .unwrap_or_else(|| panic!("molt_main missing for table init wrapper"));
            let wrapper_index = self.compile_molt_main_wrapper(
                reloc_enabled,
                main_index,
                table_init_index,
                manifest_segment,
                manifest_len as u32,
            );
            self.exports
                .export("molt_main", ExportKind::Func, wrapper_index);

            let mut ref_exported = BTreeSet::new();
            for (slot, func_index) in table_indices.iter().enumerate() {
                if slot < split_runtime_owned_slot_start
                    && slot >= split_runtime_shared_abi_slot_end
                {
                    continue;
                }
                let table_index = table_base + slot as u32;
                if ref_exported.insert(table_index) {
                    let name = format!("__molt_table_ref_{table_index}");
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
                eprintln!("[molt-wasm-import-audit] native EH disabled (MOLT_WASM_NATIVE_EH=0)");
            }

            // --- Tail call optimization audit (§3.5) ---
            eprintln!(
                "[molt-wasm-import-audit] tail calls emitted: {} (return_call instructions)",
                self.tail_calls_emitted
            );

            // --- Data segment size audit ---
            let total_data_bytes: u32 = self.data_segments.iter().map(|s| s.size).sum();
            let dedup_hits = self.data_segment_cache.len();
            eprintln!(
                "[molt-wasm-import-audit] data segments: {} segments, {} total bytes, {} dedup cache entries",
                self.data_segments.len(),
                total_data_bytes,
                dedup_hits,
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

        // --- Dead import elimination ---
        // After compilation, TrackedImportIds knows exactly which imports were
        // referenced during code emission.  Strip the unused ones from the
        // serialized module and remap all function indices.  Stripping is
        // attempted unconditionally; only the *result* is validated before
        // replacing the original binary.
        // Only applies to Auto profile in non-relocatable mode.
        // Full profile preserves all imports for maximum host compatibility;
        // Pure profile's import set is already curated and expected stable.
        // Relocatable modules are linked by wasm-ld --gc-sections instead.
        let strip_enabled = !reloc_enabled && self.options.wasm_profile == WasmProfile::Auto;
        if strip_enabled {
            let unused: BTreeSet<String> = self.import_ids.unused_names().into_iter().collect();
            if !unused.is_empty() {
                let before_len = bytes.len();
                let stripped = strip_unused_imports(bytes.clone(), &unused);
                if validate_wasm_sections(&stripped) {
                    eprintln!(
                        "[molt-wasm-strip] eliminated {} unused imports, \
                         {} -> {} bytes (saved {})",
                        unused.len(),
                        before_len,
                        stripped.len(),
                        before_len.saturating_sub(stripped.len()),
                    );
                    bytes = stripped;
                } else {
                    eprintln!(
                        "[molt-wasm-strip] stripping {} unused imports produced \
                         invalid WASM; keeping original ({} bytes)",
                        unused.len(),
                        before_len,
                    );
                }
            }
        }

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
            target_has_ret: _,
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
        owned_slot_start: usize,
        shared_abi_slot_end: usize,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(8);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        for (slot, target_index) in table_indices.iter().enumerate() {
            if slot < owned_slot_start && slot >= shared_abi_slot_end {
                continue;
            }
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
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(0);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        // Set the intrinsic manifest BEFORE table init and module init.
        // This tells the runtime which intrinsics this app actually uses.
        // Use emit_data_ptr to register a relocation so wasm-ld adjusts the
        // pointer after linking (raw I64Const would become stale).
        if manifest_len > 0 {
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, manifest_segment);
            func.instruction(&Instruction::I64Const(i64::from(manifest_len)));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["set_intrinsic_manifest"],
            );
            func.instruction(&Instruction::Drop);
        }
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
        if is_production_lir_wasm_fast_path_name(&func_ir.name)
            && !ctx.escaped_callable_targets.contains(&func_ir.name)
            && let Some(lir_output) = ctx.lir_fast_outputs.get(&func_ir.name)
        {
            let mut func = Function::new_with_locals_types(lir_output.locals.clone());
            for instruction in &lir_output.instructions {
                func.instruction(instruction);
            }
            self.codes.function(&func);
            return;
        }
        let func_map = ctx.func_map;
        let func_indices = ctx.func_indices;
        let trampoline_map = ctx.trampoline_map;
        let table_base = ctx.table_base;
        let import_ids = ctx.import_ids;
        let closure_functions = ctx.closure_functions;
        let mut locals = BTreeMap::new();
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
        let read_vars: BTreeSet<String> = {
            let mut s = BTreeSet::new();
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
        let param_set: BTreeSet<String> = func_ir.params.iter().cloned().collect();

        // --- Local variable coalescing (liveness analysis) ---
        // Compute live ranges for each variable: first write -> last read.
        // Variables whose ranges don't overlap can share a WASM local,
        // reducing total local count and binary size.
        let coalesced_map: BTreeMap<String, String> = if has_non_linear_control_flow(&func_ir.ops) {
            BTreeMap::new()
        } else {
            let mut first_write: BTreeMap<String, usize> = BTreeMap::new();
            let mut last_read: BTreeMap<String, usize> = BTreeMap::new();

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
            let mut map: BTreeMap<String, String> = BTreeMap::new();

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
        let mut const_seed_seen: BTreeSet<String> = BTreeSet::new();
        let mut const_seed_locals_all: Vec<(u32, i64)> = Vec::new();
        let mut defined_vars: BTreeSet<String> = BTreeSet::new();
        let mut used_vars: BTreeSet<String> = BTreeSet::new();
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
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry("__wasm_tmp0".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I32);
                local_count += 1;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry("__wasm_tmp1".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
        }

        if needs_alloc_resolve {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry("__wasm_alloc_resolve".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I32);
                local_count += 1;
            }
        }

        for name in ["__molt_tmp0", "__molt_tmp1", "__molt_tmp2", "__molt_tmp3"] {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry(name.to_string())
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
                ..ConstantCache::default()
            }
        } else {
            ConstantCache::default()
        };

        // Extended constant cache: cache box_none(), QNAN_TAG_MASK_I64, and
        // QNAN_TAG_PTR_I64 into locals unconditionally — these large i64
        // constants (9-10 bytes each as immediates) appear dozens of times in
        // every function body.  Replacing with local.get (1-2 bytes) saves
        // 7-8 bytes per occurrence.
        let const_cache = {
            let mut cc = const_cache;
            let none_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let mask_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let ptr_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            cc.none_bits = Some(none_idx);
            cc.qnan_tag_mask = Some(mask_idx);
            cc.qnan_tag_ptr = Some(ptr_idx);
            cc
        };

        let jumpful = !stateful && saw_jump_or_label;

        // --- Tail call optimization eligibility (WASM tail calls proposal §3.5) ---
        // A function is eligible for tail call optimization when it is
        // non-stateful (stateful dispatch emits ops one-at-a-time).
        // Exception handling is checked per-call-site via try_stack
        // instead of blanket-disabling the whole function.
        let tail_call_eligible = !stateful;

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
        let mut multi_ret_tuple_vars: BTreeSet<String> = BTreeSet::new();
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

        let mut multi_ret_call_locals: BTreeMap<(String, i64), u32> = BTreeMap::new();
        let mut multi_ret_call_vars: BTreeSet<String> = BTreeSet::new();
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
        let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

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

        let exception_handler_region_indices: BTreeSet<usize> = {
            let mut label_to_op_index: BTreeMap<i64, usize> = BTreeMap::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "label" | "state_label")
                    && let Some(label_id) = op.value
                {
                    label_to_op_index.insert(label_id, idx);
                }
            }

            let mut regions = BTreeSet::new();
            let handler_labels: Vec<i64> = func_ir
                .ops
                .iter()
                .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                .collect();

            for label in handler_labels {
                let Some(&start_idx) = label_to_op_index.get(&label) else {
                    continue;
                };
                let mut nested_pushes = 0usize;
                for handler_idx in start_idx..func_ir.ops.len() {
                    let handler_op = &func_ir.ops[handler_idx];
                    regions.insert(handler_idx);
                    match handler_op.kind.as_str() {
                        "exception_push" => nested_pushes += 1,
                        "exception_pop" => {
                            if nested_pushes == 0 {
                                break;
                            }
                            nested_pushes -= 1;
                        }
                        "ret" | "ret_void" => break,
                        _ => {}
                    }
                }
            }
            regions
        };

        let mut emit_ops = |func: &mut Function,
                            ops: &[OpIR],
                            control_stack: &mut Vec<ControlKind>,
                            try_stack: &mut Vec<usize>,
                            label_stack: &mut Vec<i64>,
                            label_depths: &mut BTreeMap<i64, usize>,
                            base_idx: usize| {
            // --- RC coalescing: eliminate redundant inc_ref/dec_ref pairs ---
            let last_use_local: BTreeMap<String, usize> = {
                let mut lu = BTreeMap::new();
                for (i, op) in ops.iter().enumerate() {
                    if let Some(var) = &op.var {
                        if var != "none" {
                            lu.insert(var.clone(), i);
                        }
                    }
                    if let Some(args) = &op.args {
                        for name in args {
                            if name != "none" {
                                lu.insert(name.clone(), i);
                            }
                        }
                    }
                }
                lu
            };
            let (rc_skip_inc, _rc_skip_dec) =
                crate::passes::compute_rc_coalesce_skips(ops, &last_use_local);

            // Peephole state: track WASM locals whose raw (unboxed) integer
            // value is known at compile time.  Populated by `const` ops;
            // invalidated when a local is overwritten by a non-const op or
            // control flow diverges.
            let mut known_raw_ints: BTreeMap<u32, i64> = BTreeMap::new();

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
                        const_cache.emit_none(func);
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

                        // Use the fixed scratch slot in linear memory instead
                        // of heap-allocating an 8-byte buffer per const_str.
                        // This eliminates the per-string alloc(8) call, the
                        // handle_resolve round-trip, and the leaked
                        // intermediate object — saving ~48 bytes of heap per
                        // string constant and reducing heap pressure that can
                        // push the allocator into the output data region in
                        // the split-runtime layout.
                        let scratch_seg = ctx.const_str_scratch_segment;

                        // string_from_bytes(data_ptr: i32, len: i64, out: i32) -> i32
                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        self.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
                        emit_call(func, reloc_enabled, import_ids["string_from_bytes"]);
                        func.instruction(&Instruction::Drop);

                        // Load the string handle written by string_from_bytes.
                        let out_local = locals[out_name];
                        self.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
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

                        // Use fixed scratch slot (same as const_str).
                        let scratch_seg = ctx.const_str_scratch_segment;

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        self.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
                        emit_call(func, reloc_enabled, import_ids["bytes_from_bytes"]);
                        func.instruction(&Instruction::Drop);

                        let out_local = locals[out_name];
                        self.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
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
                        if op.type_hint.as_deref() == Some("str") {
                            // Both operands known to be strings — direct concat,
                            // skips the 8-branch dispatch in add.
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["str_concat"]);
                        } else if op.fast_int.unwrap_or(false) {
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["add"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "inplace_add" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        if op.type_hint.as_deref() == Some("str") {
                            // Both operands known to be strings — direct concat.
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["str_concat"]);
                        } else if op.fast_int.unwrap_or(false) {
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_add"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "vec_sum_int"
                    | "vec_sum_int_trusted"
                    | "vec_sum_int_range_iter"
                    | "vec_sum_int_range_iter_trusted"
                    | "vec_sum_int_range"
                    | "vec_sum_int_range_trusted"
                    | "vec_sum_float"
                    | "vec_sum_float_trusted"
                    | "vec_sum_float_range_iter"
                    | "vec_sum_float_range_iter_trusted"
                    | "vec_sum_float_range"
                    | "vec_sum_float_range_trusted"
                    | "vec_prod_int"
                    | "vec_prod_int_trusted"
                    | "vec_prod_int_range"
                    | "vec_prod_int_range_trusted"
                    | "vec_min_int"
                    | "vec_min_int_trusted"
                    | "vec_min_int_range"
                    | "vec_min_int_range_trusted"
                    | "vec_max_int"
                    | "vec_max_int_trusted"
                    | "vec_max_int_range"
                    | "vec_max_int_range_trusted" => {
                        let args_names = op.args.as_ref().unwrap();
                        let arg_locals: Vec<u32> = args_names.iter().map(|n| locals[n]).collect();
                        let out = locals[op.out.as_ref().unwrap()];
                        emit_simple_call(
                            func,
                            reloc_enabled,
                            import_ids[op.kind.as_str()],
                            &arg_locals,
                            out,
                        );
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["sub"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["mul"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_sub"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["inplace_mul"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "invert" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["invert"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["matmul"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
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
                            emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(lhs));
                            func.instruction(&Instruction::LocalGet(rhs));
                            emit_call(func, reloc_enabled, import_ids["div"]);
                            func.instruction(&Instruction::End);
                        }
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "pow" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["pow"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "trunc" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["trunc"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_eq" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["string_eq"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "is" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["is"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "not" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["not"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "abs" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["abs_builtin"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "contains" => {
                        let args = op.args.as_ref().unwrap();
                        let container = locals[&args[0]];
                        let item = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(container));
                        func.instruction(&Instruction::LocalGet(item));
                        let import_key = match op.container_type.as_deref() {
                            Some("set") | Some("frozenset") => "set_contains",
                            Some("dict") => "dict_contains",
                            Some("list") => "list_contains",
                            Some("str") => "str_contains",
                            _ => "contains",
                        };
                        emit_call(func, reloc_enabled, import_ids[import_key]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "alloc_class" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "alloc_class_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class_trusted"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "alloc_class_static" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class_static"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        // Dispatch to specialized fast-path len when container
                        // type is known, skipping the 18-type dispatch.
                        let import_key = match op.container_type.as_deref() {
                            Some("list") | Some("list_int") => "len_list",
                            Some("str") => "len_str",
                            Some("dict") => "len_dict",
                            Some("tuple") => "len_tuple",
                            Some("set") | Some("frozenset") => "len_set",
                            _ => match op.type_hint.as_deref() {
                                Some("str") => "len_str",
                                Some("dict") => "len_dict",
                                Some("set") | Some("frozenset") => "len_set",
                                _ => "len",
                            },
                        };
                        emit_call(func, reloc_enabled, import_ids[import_key]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "id" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["id"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "ord" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["ord"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "chr" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["chr"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "callargs_new" => {
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "list_new" => {
                        let empty_args_ln: Vec<String> = Vec::new();
                        let args = op.args.as_ref().unwrap_or(&empty_args_ln);
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
                    "list_int_new" => {
                        // Specialized flat i64 list: args = [count, fill_value]
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        let count = locals[&args[0]];
                        let fill = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(count));
                        func.instruction(&Instruction::LocalGet(fill));
                        emit_call(func, reloc_enabled, import_ids["list_int_new"]);
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
                        let empty_args: Vec<String> = Vec::new();
                        let args = op.args.as_ref().unwrap_or(&empty_args);
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
                        if let Some(out_name) = op.out.as_ref() {
                            let res = locals[out_name];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            // No output variable; the runtime call returns an i64
                            // that must be consumed to keep the WASM stack balanced.
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out_name) = op.out.as_ref() {
                            let res = locals[out_name];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "callargs_expand_star" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let iterable = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(iterable));
                        emit_call(func, reloc_enabled, import_ids["callargs_expand_star"]);
                        if let Some(out_name) = op.out.as_ref() {
                            let res = locals[out_name];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "callargs_expand_kwstar" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let mapping = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(mapping));
                        emit_call(func, reloc_enabled, import_ids["callargs_expand_kwstar"]);
                        if let Some(out_name) = op.out.as_ref() {
                            let res = locals[out_name];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_append" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_append"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["list_pop"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_extend" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["list_extend"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_remove"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_clear"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_copy" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_copy"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_reverse" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_reverse"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_count" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_count"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "list_index" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_index"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "tuple_from_list" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["tuple_from_list"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_new" => {
                        let empty_args_dn: Vec<String> = Vec::new();
                        let args = op.args.as_ref().unwrap_or(&empty_args_dn);
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
                        let empty_args_sn: Vec<String> = Vec::new();
                        let args = op.args.as_ref().unwrap_or(&empty_args_sn);
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
                        let empty_args_fn: Vec<String> = Vec::new();
                        let args = op.args.as_ref().unwrap_or(&empty_args_fn);
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_update" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["dict_update"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_clear"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_copy" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_copy"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_popitem" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_popitem"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_update_kwstar" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["dict_update_kwstar"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_add"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "frozenset_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_discard" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_discard"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_remove"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        emit_call(func, reloc_enabled, import_ids["set_pop"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_update"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_intersection_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_intersection_update"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_difference_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_difference_update"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "set_symdiff_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_symdiff_update"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_keys" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_keys"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_values" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_values"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_items" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_items"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "tuple_count" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(tuple));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["tuple_count"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                    "unpack_sequence" => {
                        // args[0] is the sequence, args[1..] are output variable names.
                        // op.value holds the expected element count.
                        // The sequence may be a list (from _emit_list_from_iter) or
                        // a tuple, so use the general-purpose `index` import which
                        // handles both via __getitem__.
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let expected_count = op.value.unwrap() as usize;
                        for i in 0..expected_count {
                            let out = locals[&args[1 + i]];
                            func.instruction(&Instruction::LocalGet(seq));
                            func.instruction(&Instruction::I64Const(box_int(i as i64)));
                            emit_call(func, reloc_enabled, import_ids["index"]);
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "iter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["iter"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "aiter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["aiter"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "iter_next" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["iter_next"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "anext" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["anext"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "asyncgen_new" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        emit_call(func, reloc_enabled, import_ids["asyncgen_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "asyncgen_shutdown" => {
                        emit_call(func, reloc_enabled, import_ids["asyncgen_shutdown"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "gen_send" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["generator_send"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "gen_throw" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["generator_throw"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "gen_close" => {
                        let args = op.args.as_ref().unwrap();
                        let gen_local = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(gen_local));
                        emit_call(func, reloc_enabled, import_ids["generator_close"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "is_generator" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_generator"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "is_bound_method" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_bound_method"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "is_callable" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_callable"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        // Dispatch: list_int / dict / tuple → generic
                        let import_key = match op.container_type.as_deref() {
                            Some("list_int") => "list_int_getitem",
                            Some("dict") => "dict_getitem",
                            Some("tuple") => "tuple_getitem",
                            _ => "index",
                        };
                        emit_call(func, reloc_enabled, import_ids[import_key]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "store_index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        // Dispatch: list_int / dict → generic
                        let import_key = match op.container_type.as_deref() {
                            Some("list_int") => "list_int_setitem",
                            Some("dict") => "dict_setitem",
                            _ => "store_index",
                        };
                        emit_call(func, reloc_enabled, import_ids[import_key]);
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytes_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_find"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytearray_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_find"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_find"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_format" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let spec = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(spec));
                        emit_call(func, reloc_enabled, import_ids["format_builtin"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_startswith"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytes_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_startswith"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytearray_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_startswith"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_endswith"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytes_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_endswith"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytearray_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_endswith"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_count"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytes_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_count"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytearray_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_count"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "env_get" => {
                        let args = op.args.as_ref().unwrap();
                        let key = locals[&args[0]];
                        let default = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["env_get"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "errno_constants" => {
                        emit_call(func, reloc_enabled, import_ids["errno_constants"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_join" => {
                        let args = op.args.as_ref().unwrap();
                        let sep = locals[&args[0]];
                        let items = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sep));
                        func.instruction(&Instruction::LocalGet(items));
                        emit_call(func, reloc_enabled, import_ids["string_join"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_split"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_lower" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_lower"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_upper" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_upper"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_capitalize" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_capitalize"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_strip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_strip"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_lstrip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_lstrip"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "string_rstrip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_rstrip"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytes_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_split"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytearray_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_split"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytes_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["bytes_from_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "bytearray_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["bytearray_from_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "float_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["float_from_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "intarray_from_seq" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["intarray_from_seq"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "memoryview_new" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["memoryview_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "memoryview_tobytes" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["memoryview_tobytes"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "buffer2d_matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["buffer2d_matmul"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "str_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["str_from_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "repr_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["repr_from_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "ascii_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["ascii_from_obj"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dataclass_get" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["dataclass_get"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dataclass_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(class_obj));
                        emit_call(func, reloc_enabled, import_ids["dataclass_set_class"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "class_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["class_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "class_def" => {
                        let args = op.args.as_ref().unwrap();
                        let meta = op.s_value.as_deref().expect("class_def needs s_value");
                        let mut parts = meta.split(',');
                        let nbases = parts
                            .next()
                            .and_then(|s| s.parse::<usize>().ok())
                            .expect("class_def metadata missing base count");
                        let nattrs = parts
                            .next()
                            .and_then(|s| s.parse::<usize>().ok())
                            .expect("class_def metadata missing attr count");
                        let layout_size = parts
                            .next()
                            .and_then(|s| s.parse::<i64>().ok())
                            .expect("class_def metadata missing layout size");
                        let layout_version = parts
                            .next()
                            .and_then(|s| s.parse::<i64>().ok())
                            .expect("class_def metadata missing layout version");
                        let flags = parts
                            .next()
                            .and_then(|s| s.parse::<i64>().ok())
                            .expect("class_def metadata missing flags");

                        let spill_base = ctx.class_def_spill_offset;
                        let bases_words = nbases.max(1) as u32;
                        let attrs_base = spill_base + bases_words * 8;
                        let attrs_start = 1 + nbases;

                        for (i, base_name) in args[1..1 + nbases].iter().enumerate() {
                            let base = locals[base_name];
                            func.instruction(&Instruction::I32Const(
                                (spill_base + (i as u32) * 8) as i32,
                            ));
                            func.instruction(&Instruction::LocalGet(base));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                        }

                        for i in 0..nattrs {
                            let key = locals[&args[attrs_start + i * 2]];
                            let val = locals[&args[attrs_start + i * 2 + 1]];
                            func.instruction(&Instruction::I32Const(
                                (attrs_base + (i as u32) * 16) as i32,
                            ));
                            func.instruction(&Instruction::LocalGet(key));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::I32Const(
                                (attrs_base + (i as u32) * 16 + 8) as i32,
                            ));
                            func.instruction(&Instruction::LocalGet(val));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                        }

                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::I32Const(spill_base as i32));
                        func.instruction(&Instruction::I64Const(nbases as i64));
                        func.instruction(&Instruction::I32Const(attrs_base as i32));
                        func.instruction(&Instruction::I64Const(nattrs as i64));
                        func.instruction(&Instruction::I64Const(layout_size));
                        func.instruction(&Instruction::I64Const(layout_version));
                        func.instruction(&Instruction::I64Const(flags));
                        emit_call(func, reloc_enabled, import_ids["guarded_class_def"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "class_set_base" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let base_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(base_bits));
                        emit_call(func, reloc_enabled, import_ids["class_set_base"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "class_apply_set_name" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["class_apply_set_name"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "super_new" => {
                        let args = op.args.as_ref().unwrap();
                        let type_bits = locals[&args[0]];
                        let obj_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(type_bits));
                        func.instruction(&Instruction::LocalGet(obj_bits));
                        emit_call(func, reloc_enabled, import_ids["super_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "builtin_type" => {
                        let args = op.args.as_ref().unwrap();
                        let tag = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(tag));
                        emit_call(func, reloc_enabled, import_ids["builtin_type"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "type_of" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["type_of"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "class_layout_version" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["class_layout_version"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "issubclass" => {
                        let args = op.args.as_ref().unwrap();
                        let sub = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sub));
                        func.instruction(&Instruction::LocalGet(cls));
                        emit_call(func, reloc_enabled, import_ids["issubclass"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "object_new" => {
                        emit_call(func, reloc_enabled, import_ids["object_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "classmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["classmethod_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "staticmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["staticmethod_new"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "object_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalGet(class_obj));
                        emit_call(func, reloc_enabled, import_ids["object_set_class"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "get_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["get_attr_name"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "has_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["has_attr_name"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "del_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["del_attr_name"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "store" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_old = locals["__wasm_tmp1"];

                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
                        func.instruction(&Instruction::I64Eq);

                        func.instruction(&Instruction::LocalGet(val));
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                            const_cache.emit_none(func);
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

                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                            const_cache.emit_none(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
                        func.instruction(&Instruction::I64Eq);

                        func.instruction(&Instruction::LocalGet(val));
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                            const_cache.emit_none(func);
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
                        const_cache.emit_qnan_tag_mask(func);
                        func.instruction(&Instruction::I64And);
                        const_cache.emit_qnan_tag_ptr(func);
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
                            const_cache.emit_none(func);
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
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                        let res = if let Some(out) = op.out.as_ref() {
                            let r = locals[out];
                            func.instruction(&Instruction::LocalSet(r));
                            r
                        } else {
                            func.instruction(&Instruction::Drop);
                            0
                        };
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
                        let func_idx = *func_indices.get(target_name).unwrap_or_else(|| {
                            panic!(
                                "call target not found: '{}' in func '{}'",
                                target_name, func_ir.name
                            )
                        });
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
                        // Direct call: push args, call function, store result.
                        // The recursion guard + trace_enter/exit overhead
                        // was causing the return value to be lost (the
                        // if/else block left `out` as None even on the
                        // success path in some WASM engines).  Module chunk
                        // calls and devirtualized calls now use a flat
                        // sequence; CHECK_EXCEPTION after the call catches
                        // any exception the callee raises.
                        for arg_name in args_names {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        emit_call(func, reloc_enabled, func_idx);
                        func.instruction(&Instruction::LocalSet(out));
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
                        //   1. The function is eligible (non-stateful)
                        //   2. The very next op is `ret`
                        //   3. The ret's var matches this call's output
                        //   4. There are no cleanup ops (dec_ref) between call and return
                        //   5. We are not inside a try block (return_call would
                        //      skip the exception handler)
                        //   6. Caller and callee have the same arity — return_call
                        //      requires the stack to match the callee's full param
                        //      list, which differs from call+return.
                        let is_tail_call = tail_call_eligible
                            && try_stack.is_empty()
                            && rel_idx + 1 < ops.len()
                            && ops[rel_idx + 1].kind == "ret"
                            && ops[rel_idx + 1].var.as_deref() == Some(out_name.as_str())
                            // Exclude calls to multi-return candidates: return_call
                            // would forward N values but the caller's type signature
                            // expects a single i64 return, causing an ABI mismatch.
                            && !multi_return_candidates.contains_key(target_name)
                            // Exclude chunk calls: the stub may pass fewer args than
                            // the chunk expects, causing return_call stack underflow.
                            && !target_name.contains("__molt_chunk_")
                            // Exclude calls where caller arity != callee param count.
                            // return_call requires exactly the callee's param count
                            // on the stack; a regular call+return handles mismatches.
                            && args_names.len() == func_ir.params.len();

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
                        if !rc_skip_inc.contains(&rel_idx) {
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
                        } else if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            // RC coalesced: still alias output to input.
                            let args_names = op.args.as_ref().unwrap();
                            let src_name = args_names.first().unwrap();
                            let src = locals[src_name];
                            let out = locals[out_name];
                            func.instruction(&Instruction::LocalGet(src));
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "dec_ref" | "release" => {
                        if !rc_skip_inc.contains(&rel_idx) {
                            let args_names =
                                op.args.as_ref().expect("dec_ref/release args missing");
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
                                const_cache.emit_none(func);
                                func.instruction(&Instruction::LocalSet(out));
                            }
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
                        let escaped_target = ctx.escaped_callable_targets.contains(target_name);
                        let func_idx = *func_indices
                            .get(target_name)
                            .expect("call_guarded target not found");
                        let table_slot = func_map[target_name];
                        let table_idx = table_base + table_slot;
                        if escaped_target {
                            func.instruction(&Instruction::LocalGet(callee_bits));
                            emit_call(func, reloc_enabled, import_ids["is_function_obj"]);
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            let code_id = op.value.unwrap_or(0);
                            func.instruction(&Instruction::I64Const(code_id));
                            emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                            func.instruction(&Instruction::Drop);
                            let spill_base = ctx.call_func_spill_offset;
                            for (i, arg_name) in args_names[1..].iter().enumerate() {
                                let arg = locals[arg_name];
                                func.instruction(&Instruction::I32Const(
                                    (spill_base + (i as u32) * 8) as i32,
                                ));
                                func.instruction(&Instruction::LocalGet(arg));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                            }
                            func.instruction(&Instruction::LocalGet(callee_bits));
                            func.instruction(&Instruction::I64Const(spill_base as i64));
                            func.instruction(&Instruction::I64Const(arity as i64));
                            func.instruction(&Instruction::I64Const(code_id));
                            emit_call(func, reloc_enabled, import_ids["call_func_dispatch"]);
                            func.instruction(&Instruction::LocalSet(out));
                            emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                            func.instruction(&Instruction::Drop);
                            emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                            func.instruction(&Instruction::Else);
                            // Recursion guard failed — exception is already pending.
                            // Return immediately so the pending RecursionError
                            // propagates to the caller instead of being silently
                            // swallowed as None (which caused TypeError downstream).
                            const_cache.emit_none(func);
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
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
                            continue;
                        }
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
                        // For closure functions, extract the closure environment
                        // from the callee object and push it as the leading arg.
                        // The WASM signature of closure functions is
                        //   (closure_env, arg1, arg2, …) → i64
                        // so we must prepend the env before the user arguments.
                        if closure_functions.contains(target_name) {
                            func.instruction(&Instruction::LocalGet(callee_bits));
                            emit_call(func, reloc_enabled, import_ids["function_closure_bits"]);
                        }
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
                        // Recursion guard failed — exception is already pending.
                        // Return immediately so the pending RecursionError
                        // propagates to the caller instead of being silently
                        // swallowed as None (which caused TypeError downstream).
                        const_cache.emit_none(func);
                        func.instruction(&Instruction::Return);
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "call_func" => {
                        // Outlined: spill args to linear memory, then delegate
                        // to molt_call_func_dispatch runtime helper.
                        let args_names = op.args.as_ref().unwrap();
                        let func_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let nargs = args_names.len().saturating_sub(1);
                        let spill_base = ctx.call_func_spill_offset;

                        // Spill each arg to consecutive i64 slots in linear memory.
                        for (i, arg_name) in args_names[1..].iter().enumerate() {
                            let arg = locals[arg_name];
                            // addr (i32) = spill_base + i * 8
                            func.instruction(&Instruction::I32Const(
                                (spill_base + (i as u32) * 8) as i32,
                            ));
                            func.instruction(&Instruction::LocalGet(arg));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                        }

                        // Push args: func_bits, args_ptr, nargs, code_id
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::I64Const(spill_base as i64));
                        func.instruction(&Instruction::I64Const(nargs as i64));
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        emit_call(func, reloc_enabled, import_ids["call_func_dispatch"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "invoke_ffi" => {
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
                        let invoke_bridge_lane = op.s_value.as_deref() == Some("bridge");
                        let call_site_label = if invoke_bridge_lane {
                            "invoke_ffi_bridge"
                        } else {
                            "invoke_ffi_deopt"
                        };
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            call_site_label,
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        let require_bridge_cap = if invoke_bridge_lane { 1 } else { 0 };
                        func.instruction(&Instruction::I64Const(box_bool(require_bridge_cap)));
                        emit_call(func, reloc_enabled, import_ids["invoke_ffi_ic"]);
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

                        // Fast-path: dispatch known bound-method patterns
                        // directly without callargs allocation or IC lookup.
                        let fast_dispatched = if let Some(sv) = op.s_value.as_deref() {
                            let arity = args_names.len().saturating_sub(1);
                            match sv {
                                "BoundMethod:list:append" if arity == 1 => {
                                    let arg = locals[&args_names[1]];
                                    func.instruction(&Instruction::LocalGet(method_bits));
                                    func.instruction(&Instruction::LocalGet(arg));
                                    emit_call(func, reloc_enabled, import_ids["fast_list_append"]);
                                    true
                                }
                                "BoundMethod:str:join" if arity == 1 => {
                                    let arg = locals[&args_names[1]];
                                    func.instruction(&Instruction::LocalGet(method_bits));
                                    func.instruction(&Instruction::LocalGet(arg));
                                    emit_call(func, reloc_enabled, import_ids["fast_str_join"]);
                                    true
                                }
                                "BoundMethod:dict:get" if arity == 2 => {
                                    let key = locals[&args_names[1]];
                                    let default = locals[&args_names[2]];
                                    func.instruction(&Instruction::LocalGet(method_bits));
                                    func.instruction(&Instruction::LocalGet(key));
                                    func.instruction(&Instruction::LocalGet(default));
                                    emit_call(func, reloc_enabled, import_ids["fast_dict_get"]);
                                    true
                                }
                                _ => false,
                            }
                        } else {
                            false
                        };

                        if !fast_dispatched {
                            // Generic path: allocate callargs and dispatch via IC.
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
                        }
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "chan_new" => {
                        let args = op.args.as_ref().unwrap();
                        let cap = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cap));
                        emit_call(func, reloc_enabled, import_ids["chan_new"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "module_cache_get" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_cache_get"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "module_import" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_import"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "module_cache_set" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        let module = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(module));
                        emit_call(func, reloc_enabled, import_ids["module_cache_set"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        }
                    }
                    "module_get_attr" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_attr"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "module_get_global" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_global"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "module_import_star" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        let dst = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalGet(dst));
                        emit_call(func, reloc_enabled, import_ids["module_import_star"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        let res = if let Some(out) = op.out.as_ref() {
                            let r = locals[out];
                            func.instruction(&Instruction::LocalSet(r));
                            r
                        } else {
                            func.instruction(&Instruction::Drop);
                            0
                        };
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
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "context_enter" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        emit_call(func, reloc_enabled, import_ids["context_enter"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "context_exit" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_exit"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "context_unwind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_unwind"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "context_depth" => {
                        emit_call(func, reloc_enabled, import_ids["context_depth"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "context_unwind_to" => {
                        let args = op.args.as_ref().unwrap();
                        let depth = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(depth));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_unwind_to"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "context_closing" => {
                        let args = op.args.as_ref().unwrap();
                        let payload = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(payload));
                        emit_call(func, reloc_enabled, import_ids["context_closing"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_push" => {
                        if native_eh_enabled {
                            // Native EH: no-op; WASM runtime manages handler stack.
                            const_cache.emit_none(func);
                        } else {
                            emit_call(func, reloc_enabled, import_ids["exception_push"]);
                        }
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_pop" => {
                        if native_eh_enabled {
                            const_cache.emit_none(func);
                        } else {
                            emit_call(func, reloc_enabled, import_ids["exception_pop"]);
                        }
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_stack_clear" => {
                        emit_call(func, reloc_enabled, import_ids["exception_stack_clear"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_last" => {
                        emit_call(func, reloc_enabled, import_ids["exception_last"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_new" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        let args_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(kind));
                        func.instruction(&Instruction::LocalGet(args_bits));
                        emit_call(func, reloc_enabled, import_ids["exception_new"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_new_from_class" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let args_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(args_bits));
                        emit_call(func, reloc_enabled, import_ids["exception_new_from_class"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exceptiongroup_match" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let matcher = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(matcher));
                        emit_call(func, reloc_enabled, import_ids["exceptiongroup_match"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exceptiongroup_combine" => {
                        let args = op.args.as_ref().unwrap();
                        let items = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(items));
                        emit_call(func, reloc_enabled, import_ids["exceptiongroup_combine"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_clear" => {
                        emit_call(func, reloc_enabled, import_ids["exception_clear"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_kind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_kind"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_class" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(kind));
                        emit_call(func, reloc_enabled, import_ids["exception_class"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_message" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_message"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_set_cause" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let cause = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(cause));
                        emit_call(func, reloc_enabled, import_ids["exception_set_cause"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_set_value" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let value = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(value));
                        emit_call(func, reloc_enabled, import_ids["exception_set_value"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_context_set" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_context_set"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "exception_set_last" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_set_last"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                            if let Some(ref out) = op.out {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                // raise with no output — drop the result from the stack
                                func.instruction(&Instruction::Drop);
                            }
                        }
                    }
                    "bridge_unavailable" => {
                        let args = op.args.as_ref().unwrap();
                        let msg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(msg));
                        emit_call(func, reloc_enabled, import_ids["bridge_unavailable"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "file_open" => {
                        let args = op.args.as_ref().unwrap();
                        let path = locals[&args[0]];
                        let mode = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(path));
                        func.instruction(&Instruction::LocalGet(mode));
                        emit_call(func, reloc_enabled, import_ids["file_open"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "file_read" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let size = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(size));
                        emit_call(func, reloc_enabled, import_ids["file_read"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "file_write" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let data = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(data));
                        emit_call(func, reloc_enabled, import_ids["file_write"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "file_close" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(handle));
                        emit_call(func, reloc_enabled, import_ids["file_close"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "file_flush" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(handle));
                        emit_call(func, reloc_enabled, import_ids["file_flush"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_token_new" => {
                        let args = op.args.as_ref().unwrap();
                        let parent = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(parent));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_new"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_token_clone" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_clone"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_token_drop" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_drop"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_token_cancel" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_cancel"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "future_cancel" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_cancel"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "future_cancel_msg" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let msg = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(msg));
                        emit_call(func, reloc_enabled, import_ids["future_cancel_msg"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "future_cancel_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_cancel_clear"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "promise_new" => {
                        emit_call(func, reloc_enabled, import_ids["promise_new"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "promise_set_result" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let result = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(result));
                        emit_call(func, reloc_enabled, import_ids["promise_set_result"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "promise_set_exception" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["promise_set_exception"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "task_register_token_owned" => {
                        let args = op.args.as_ref().unwrap();
                        let task = locals[&args[0]];
                        let token = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(task));
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_token_set_current" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_set_current"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_token_get_current" => {
                        emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancelled" => {
                        emit_call(func, reloc_enabled, import_ids["cancelled"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "cancel_current" => {
                        emit_call(func, reloc_enabled, import_ids["cancel_current"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "block_on" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        emit_call(func, reloc_enabled, import_ids["block_on"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
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
                                const_cache.emit_none(func);
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
                    "br_if" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        let target = op.value.expect("br_if missing label");
                        let depth = label_depths
                            .get(&target)
                            .map(|idx| control_stack.len().saturating_sub(1 + idx))
                            .unwrap_or_else(|| {
                                panic!("br_if target {} missing label block", target)
                            });
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::BrIf(depth as u32));
                    }
                    "if" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        let truthy_import = if op.type_hint.as_deref() == Some("bool") {
                            "is_truthy_bool"
                        } else if op.fast_int.unwrap_or(false)
                            || op.type_hint.as_deref() == Some("int")
                        {
                            "is_truthy_int"
                        } else {
                            "is_truthy"
                        };
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids[truthy_import]);
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
                            const_cache.emit_none(func);
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
                        } else if exception_handler_region_indices.contains(&op_idx) {
                            // Handler bodies work against the currently pending
                            // exception. Re-polling before exception_clear would
                            // re-branch out of the handler and skip its body.
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
                    | "br_if"
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
            let exception_handler_region_indices: std::collections::BTreeSet<usize> = {
                let mut regions = std::collections::BTreeSet::new();
                let handler_labels: Vec<i64> = func_ir
                    .ops
                    .iter()
                    .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                    .collect();
                for label in handler_labels {
                    let Some(&start_idx) = label_to_index.get(&label) else {
                        continue;
                    };
                    let mut nested_pushes = 0usize;
                    for handler_idx in start_idx..op_count {
                        let handler_op = &func_ir.ops[handler_idx];
                        regions.insert(handler_idx);
                        match handler_op.kind.as_str() {
                            "exception_push" => nested_pushes += 1,
                            "exception_pop" => {
                                if nested_pushes == 0 {
                                    break;
                                }
                                nested_pushes -= 1;
                            }
                            "ret" | "ret_void" => break,
                            _ => {}
                        }
                    }
                }
                regions
            };
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
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Or);
            func.instruction(&Instruction::LocalSet(self_local));

            func.instruction(&Instruction::LocalGet(self_ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            emit_call(func, reloc_enabled, import_ids["obj_get_state"]);
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
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            if let Some(encoded) = resume_encoded {
                                func.instruction(&Instruction::I64Const(encoded));
                            } else {
                                func.instruction(&Instruction::I64Const(resume_state_id));
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
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
                            let truthy_import = if op.type_hint.as_deref() == Some("bool") {
                                "is_truthy_bool"
                            } else if op.fast_int.unwrap_or(false)
                                || op.type_hint.as_deref() == Some("int")
                            {
                                "is_truthy_int"
                            } else {
                                "is_truthy"
                            };
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids[truthy_import]);
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
                        "br_if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let Some(target_label) = op.value else {
                                eprintln!(
                                    "WASM lowering warning: br_if missing label in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                continue;
                            };
                            let Some(target_idx) = label_to_index.get(&target_label).copied()
                            else {
                                eprintln!(
                                    "WASM lowering warning: unknown br_if label {} in {} at op {}; falling through",
                                    target_label, func_ir.name, idx
                                );
                                continue;
                            };
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(target_idx as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
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
                            } else if exception_handler_region_indices.contains(&idx) {
                                // Exception-handler regions operate on the currently
                                // pending exception. Re-polling here would immediately
                                // re-branch back into the same handler before
                                // exception_clear/print/cleanup can run.
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
                                const_cache.emit_none(func);
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
            const_cache.emit_none(func);
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
            let exception_handler_region_indices: std::collections::BTreeSet<usize> = {
                let mut regions = std::collections::BTreeSet::new();
                let handler_labels: Vec<i64> = func_ir
                    .ops
                    .iter()
                    .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                    .collect();
                for label in handler_labels {
                    let Some(&start_idx) = label_to_index.get(&label) else {
                        continue;
                    };
                    let mut nested_pushes = 0usize;
                    for handler_idx in start_idx..op_count {
                        let handler_op = &func_ir.ops[handler_idx];
                        regions.insert(handler_idx);
                        match handler_op.kind.as_str() {
                            "exception_push" => nested_pushes += 1,
                            "exception_pop" => {
                                if nested_pushes == 0 {
                                    break;
                                }
                                nested_pushes -= 1;
                            }
                            "ret" | "ret_void" => break,
                            _ => {}
                        }
                    }
                }
                regions
            };

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();
            let mut label_stack: Vec<i64> = Vec::new();
            let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

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
                            let truthy_import = if op.type_hint.as_deref() == Some("bool") {
                                "is_truthy_bool"
                            } else if op.fast_int.unwrap_or(false)
                                || op.type_hint.as_deref() == Some("int")
                            {
                                "is_truthy_int"
                            } else {
                                "is_truthy"
                            };
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids[truthy_import]);
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
                        "br_if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let Some(target_label) = op.value else {
                                eprintln!(
                                    "WASM lowering warning: br_if missing label in {} at op {}; falling through",
                                    func_ir.name, idx
                                );
                                continue;
                            };
                            let Some(target_idx) = label_to_index.get(&target_label).copied()
                            else {
                                eprintln!(
                                    "WASM lowering warning: unknown br_if label {} in {} at op {}; falling through",
                                    target_label, func_ir.name, idx
                                );
                                continue;
                            };
                            func.instruction(&Instruction::LocalGet(cond));
                            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(target_idx as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
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
                            } else if exception_handler_region_indices.contains(&idx) {
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
                                const_cache.emit_none(func);
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
            const_cache.emit_none(func);
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else {
            let func = &mut func;
            let mut jump_labels: BTreeSet<i64> = BTreeSet::new();
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
            // Plain functions can legally rely on Python's implicit `None`
            // return. Match the stateful/jumpful lowering paths instead of
            // falling off the end of an i64-returning WASM function.
            const_cache.emit_none(func);
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

/// Emit a simple N-arg import call: push args, call, store result.
fn emit_simple_call(
    func: &mut Function,
    reloc_enabled: bool,
    import_id: u32,
    arg_locals: &[u32],
    out_local: u32,
) {
    for &arg in arg_locals {
        func.instruction(&Instruction::LocalGet(arg));
    }
    emit_call(func, reloc_enabled, import_id);
    func.instruction(&Instruction::LocalSet(out_local));
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

// ---------------------------------------------------------------------------
// Dead import elimination (post-serialization rewrite)
// ---------------------------------------------------------------------------
//
// After the WASM module is serialized, this pass removes function imports that
// were registered but never referenced during code emission.  Removing imports
// shifts all function indices above the removed slots, so we must remap every
// function-index reference in the module:
//
//   - Import section:  rebuilt without the dead entries
//   - Code section:    call / return_call / ref.func operands remapped
//   - Element section: function index entries remapped
//   - Export section:   function index entries remapped
//   - Start section:   function index remapped (if present)
//
// The approach: parse the binary section by section using `wasmparser`,
// rebuild affected sections using `wasm_encoder`, and copy unchanged
// sections verbatim.

/// Read a single unsigned LEB128 value from `data[offset..]`.
/// Returns `(value, new_offset)`.
fn read_u32_leb128(data: &[u8], mut offset: usize) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    loop {
        let byte = data[offset];
        offset += 1;
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (result, offset)
}

/// Encode a u32 as unsigned LEB128 and append to `out`.
fn encode_u32_leb128(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Strip unused function imports from a serialized WASM module.
///
/// `unused_names` contains the import field names (within the "molt_runtime"
/// module) that should be removed.  Returns a new WASM binary with those
/// imports excised and all function indices remapped accordingly.
fn strip_unused_imports(bytes: Vec<u8>, unused_names: &BTreeSet<String>) -> Vec<u8> {
    // Phase 1: Parse the import section to build the old→new index remap.
    let mut func_import_count: u32 = 0;
    let mut remap: Vec<u32> = Vec::new();
    let mut removed_count: u32 = 0;

    {
        let mut parse_ok = true;
        for payload in Parser::new(0).parse_all(&bytes) {
            let payload = match payload {
                Ok(p) => p,
                Err(_) => {
                    parse_ok = false;
                    break;
                }
            };
            if let Payload::ImportSection(reader) = payload {
                let mut new_idx: u32 = 0;
                for import in reader.into_imports().flatten() {
                    let is_func = matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_));
                    if is_func {
                        func_import_count += 1;
                        if unused_names.contains(import.name) {
                            remap.push(u32::MAX);
                            removed_count += 1;
                        } else {
                            remap.push(new_idx);
                            new_idx += 1;
                        }
                    }
                }
                break;
            }
        }
        if !parse_ok {
            return bytes;
        }
    }

    if removed_count == 0 {
        return bytes;
    }

    let remap_func_index = |old: u32| -> u32 {
        if old < func_import_count {
            remap[old as usize]
        } else {
            old - removed_count
        }
    };

    // Phase 2: Rebuild the module by reading raw section bytes.
    // WASM binary format: magic (4 bytes) + version (4 bytes) + sections.
    // Each section: section_id (1 byte) + u32 LEB128 size + content bytes.
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..8]); // header

    let mut pos = 8usize;
    while pos < bytes.len() {
        let section_id = bytes[pos];
        pos += 1;
        let (section_size, content_start) = read_u32_leb128(&bytes, pos);
        let content_end = content_start + section_size as usize;
        let section_bytes = &bytes[content_start..content_end];

        match section_id {
            // Import section (2): rebuild without dead entries.
            2 => {
                let mut section = ImportSection::new();
                // Re-parse just this section.
                for payload in Parser::new(0).parse_all(&bytes) {
                    let Ok(payload) = payload else { break };
                    if let Payload::ImportSection(reader) = payload {
                        for import in reader.into_imports().flatten() {
                            let is_func = matches!(import.ty, TypeRef::Func(_));
                            if is_func && unused_names.contains(import.name) {
                                continue;
                            }
                            let entity_ty = match import.ty {
                                TypeRef::Func(idx) => EntityType::Function(idx),
                                TypeRef::Table(t) => EntityType::Table(wasm_encoder::TableType {
                                    element_type: convert_ref_type(t.element_type),
                                    table64: t.table64,
                                    minimum: t.initial.into(),
                                    maximum: t.maximum.map(Into::into),
                                    shared: t.shared,
                                }),
                                TypeRef::Memory(m) => EntityType::Memory(MemoryType {
                                    minimum: m.initial,
                                    maximum: m.maximum,
                                    memory64: m.memory64,
                                    shared: m.shared,
                                    page_size_log2: m.page_size_log2,
                                }),
                                TypeRef::Global(g) => {
                                    EntityType::Global(wasm_encoder::GlobalType {
                                        val_type: convert_val_type(g.content_type),
                                        mutable: g.mutable,
                                        shared: g.shared,
                                    })
                                }
                                TypeRef::Tag(t) => EntityType::Tag(TagType {
                                    kind: TagKind::Exception,
                                    func_type_idx: t.func_type_idx,
                                }),
                                TypeRef::FuncExact(idx) => EntityType::Function(idx),
                            };
                            section.import(import.module, import.name, entity_ty);
                        }
                        break;
                    }
                }
                out.push(2); // import section id
                section.encode(&mut out);
            }

            // Export section (7): remap function indices.
            7 => {
                let mut section = ExportSection::new();
                for payload in Parser::new(0).parse_all(&bytes) {
                    let Ok(payload) = payload else { break };
                    if let Payload::ExportSection(reader) = payload {
                        for export in reader.into_iter().flatten() {
                            let kind = match export.kind {
                                ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
                                ExternalKind::Table => ExportKind::Table,
                                ExternalKind::Memory => ExportKind::Memory,
                                ExternalKind::Global => ExportKind::Global,
                                ExternalKind::Tag => ExportKind::Tag,
                            };
                            let index = if matches!(
                                export.kind,
                                ExternalKind::Func | ExternalKind::FuncExact
                            ) {
                                remap_func_index(export.index)
                            } else {
                                export.index
                            };
                            section.export(export.name, kind, index);
                        }
                        break;
                    }
                }
                out.push(7); // export section id
                section.encode(&mut out);
            }

            // Element section (9): remap function indices.
            9 => {
                let mut section = ElementSection::new();
                for payload in Parser::new(0).parse_all(&bytes) {
                    let Ok(payload) = payload else { break };
                    if let Payload::ElementSection(reader) = payload {
                        for element in reader.into_iter().flatten() {
                            if let ElementItems::Functions(funcs) = element.items {
                                let indices: Vec<u32> = funcs
                                    .into_iter()
                                    .flatten()
                                    .map(|idx| remap_func_index(idx))
                                    .collect();
                                match element.kind {
                                    wasmparser::ElementKind::Active {
                                        table_index,
                                        offset_expr,
                                    } => {
                                        let mut ops = offset_expr.get_operators_reader();
                                        let offset_val = if let Ok(op) = ops.read() {
                                            match op {
                                                Operator::I32Const { value } => value,
                                                _ => 0,
                                            }
                                        } else {
                                            0
                                        };
                                        let c = ConstExpr::i32_const(offset_val);
                                        let table = table_index.filter(|&t| t != 0);
                                        section.segment(ElementSegment {
                                            mode: ElementMode::Active {
                                                table: table,
                                                offset: &c,
                                            },
                                            elements: Elements::Functions(Cow::Owned(indices)),
                                        });
                                    }
                                    wasmparser::ElementKind::Passive => {
                                        section.segment(ElementSegment {
                                            mode: ElementMode::Passive,
                                            elements: Elements::Functions(Cow::Owned(indices)),
                                        });
                                    }
                                    wasmparser::ElementKind::Declared => {
                                        section.segment(ElementSegment {
                                            mode: ElementMode::Declared,
                                            elements: Elements::Functions(Cow::Owned(indices)),
                                        });
                                    }
                                };
                            }
                        }
                        break;
                    }
                }
                out.push(9); // element section id
                section.encode(&mut out);
            }

            // Code section (10): remap function indices in instructions.
            10 => {
                let new_code = remap_code_section(section_bytes, &remap_func_index);
                out.push(10);
                encode_u32_leb128(new_code.len() as u32, &mut out);
                out.extend_from_slice(&new_code);
            }

            // Start section (8): remap start function index.
            8 => {
                let (old_idx, _) = read_u32_leb128(section_bytes, 0);
                let new_idx = remap_func_index(old_idx);
                let mut body = Vec::new();
                encode_u32_leb128(new_idx, &mut body);
                out.push(8);
                encode_u32_leb128(body.len() as u32, &mut out);
                out.extend_from_slice(&body);
            }

            // All other sections: copy verbatim.
            _ => {
                out.push(section_id);
                // Copy the original LEB128 size + content.
                out.extend_from_slice(&bytes[pos..content_end]);
            }
        }

        pos = content_end;
    }

    if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1") {
        eprintln!(
            "[molt-wasm-import-strip] eliminated {removed_count} unused imports \
             ({func_import_count} → {}), binary {} → {} bytes (saved {} bytes)",
            func_import_count - removed_count,
            bytes.len(),
            out.len(),
            bytes.len().saturating_sub(out.len()),
        );
    }

    out
}

/// Validate that a WASM binary has well-formed section structure.
/// Returns true if all section IDs are valid and sizes don't overflow.
fn validate_wasm_sections(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    if &bytes[0..4] != b"\x00asm" {
        return false;
    }
    let mut pos = 8usize;
    while pos < bytes.len() {
        let section_id = bytes[pos];
        // Valid: 0 (custom), 1-12 (standard), 13 (tag from exception handling).
        if section_id > 13 {
            return false;
        }
        pos += 1;
        if pos >= bytes.len() {
            return false;
        }
        let (size, new_pos) = read_u32_leb128(bytes, pos);
        pos = new_pos + size as usize;
        if pos > bytes.len() {
            return false;
        }
    }
    pos == bytes.len()
}

/// Convert a wasmparser RefType to a wasm_encoder RefType.
fn convert_ref_type(ty: wasmparser::RefType) -> RefType {
    if ty.is_func_ref() {
        RefType::FUNCREF
    } else if ty.is_extern_ref() {
        RefType::EXTERNREF
    } else {
        RefType::FUNCREF // fallback
    }
}

/// Convert a wasmparser ValType to a wasm_encoder ValType.
fn convert_val_type(ty: wasmparser::ValType) -> ValType {
    match ty {
        wasmparser::ValType::I32 => ValType::I32,
        wasmparser::ValType::I64 => ValType::I64,
        wasmparser::ValType::F32 => ValType::F32,
        wasmparser::ValType::F64 => ValType::F64,
        wasmparser::ValType::V128 => ValType::V128,
        wasmparser::ValType::Ref(r) => ValType::Ref(convert_ref_type(r)),
    }
}

/// Rewrite the code section content (after the section header) with remapped
/// function indices.  Returns the new section body (count + function bodies).
///
/// WASM opcodes that reference function indices:
///   - call (0x10): u32 function_index
///   - return_call (0x12): u32 function_index
///   - ref.func (0xD2): u32 function_index
fn remap_code_section(section_content: &[u8], remap: &dyn Fn(u32) -> u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(section_content.len());

    // The section content starts with the function count.
    let (count, mut offset) = read_u32_leb128(section_content, 0);
    encode_u32_leb128(count, &mut out);

    // Process each function body.
    for _ in 0..count {
        let (body_size, body_start) = read_u32_leb128(section_content, offset);
        let body_end = body_start + body_size as usize;
        let body = &section_content[body_start..body_end];

        // Rewrite function indices within this function body.
        let new_body = remap_function_body(body, remap);

        encode_u32_leb128(new_body.len() as u32, &mut out);
        out.extend_from_slice(&new_body);

        offset = body_end;
    }

    out
}

/// Rewrite a single function body, remapping call/return_call/ref.func indices.
///
/// Function body format: locals declarations + instruction sequence.
/// We skip over the locals section and scan the instruction bytes.
fn remap_function_body(body: &[u8], remap: &dyn Fn(u32) -> u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut pos: usize = 0;

    // Skip local declarations.
    let (local_decl_count, new_pos) = read_u32_leb128(body, pos);
    // Copy the local declarations verbatim.
    let locals_start = pos;
    pos = new_pos;
    for _ in 0..local_decl_count {
        // Each declaration: count (u32 LEB128) + type (1 byte).
        let (_count, p) = read_u32_leb128(body, pos);
        pos = p + 1; // +1 for the type byte
    }
    // Copy everything up to here (local declarations).
    out.extend_from_slice(&body[locals_start..pos]);

    // Now scan the instruction stream.
    while pos < body.len() {
        let opcode = body[pos];
        match opcode {
            // call (0x10) or return_call (0x12): remap the function index.
            0x10 | 0x12 => {
                out.push(opcode);
                pos += 1;
                let (old_idx, new_pos) = read_u32_leb128(body, pos);
                let new_idx = remap(old_idx);
                encode_u32_leb128(new_idx, &mut out);
                pos = new_pos;
            }
            // ref.func (0xD2): remap the function index.
            0xD2 => {
                out.push(opcode);
                pos += 1;
                let (old_idx, new_pos) = read_u32_leb128(body, pos);
                let new_idx = remap(old_idx);
                encode_u32_leb128(new_idx, &mut out);
                pos = new_pos;
            }
            // All other opcodes: copy byte by byte.
            // For multi-byte opcodes (0xFC prefix, 0xFE prefix), we need to
            // be careful not to accidentally interpret operands as opcodes.
            // However, function index operands only appear in the three
            // opcodes above, so we just need to skip operands correctly.
            //
            // Fortunately, WASM instruction encoding is self-describing:
            // each opcode has a fixed operand format.  We handle the common
            // cases and for anything else, copy byte-by-byte (which is safe
            // because no other single-byte opcode's operand looks like 0x10,
            // 0x12, or 0xD2 in a way that would be misinterpreted — WASM
            // LEB128 operands are self-delimiting).
            //
            // IMPORTANT: We must handle opcodes with LEB128 operands to avoid
            // misinterpreting operand bytes as opcodes.
            _ => {
                out.push(opcode);
                pos += 1;
                // Copy operands for known opcodes to avoid misparse.
                match opcode {
                    // Control flow with block types
                    0x02 | 0x03 | 0x04 => {
                        // block/loop/if: blocktype (signed LEB128)
                        copy_sleb128(body, &mut pos, &mut out);
                    }
                    // br / br_if: label index (u32 LEB128)
                    0x0C | 0x0D => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // br_table: vec(label) + default_label
                    0x0E => {
                        let (count, new_pos) = read_u32_leb128(body, pos);
                        // Re-encode count
                        let start = pos;
                        pos = new_pos;
                        out.extend_from_slice(&body[start..pos]);
                        // Skip count+1 label indices (but we already wrote count).
                        // Actually, we need to just copy them. Let me redo this.
                        // We wrote count in the extend above. Now copy count+1 labels.
                        for _ in 0..=count {
                            copy_uleb128(body, &mut pos, &mut out);
                        }
                    }
                    // call_indirect: type_index + table_index
                    0x11 => {
                        copy_uleb128(body, &mut pos, &mut out); // type_index
                        copy_uleb128(body, &mut pos, &mut out); // table_index
                    }
                    // Variable access: local.get/set/tee (0x20-0x22)
                    0x20 | 0x21 | 0x22 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // Global access: global.get/set (0x23-0x24)
                    0x23 | 0x24 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // Memory instructions (0x28-0x3E): memarg (align + offset)
                    0x28..=0x3E => {
                        copy_uleb128(body, &mut pos, &mut out); // align
                        copy_uleb128(body, &mut pos, &mut out); // offset
                    }
                    // memory.size / memory.grow (0x3F, 0x40): memory index
                    0x3F | 0x40 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // i32.const (0x41): signed LEB128
                    0x41 => {
                        copy_sleb128(body, &mut pos, &mut out);
                    }
                    // i64.const (0x42): signed LEB128 (64-bit)
                    0x42 => {
                        copy_sleb128_64(body, &mut pos, &mut out);
                    }
                    // f32.const (0x43): 4 bytes
                    0x43 => {
                        out.extend_from_slice(&body[pos..pos + 4]);
                        pos += 4;
                    }
                    // f64.const (0x44): 8 bytes
                    0x44 => {
                        out.extend_from_slice(&body[pos..pos + 8]);
                        pos += 8;
                    }
                    // 0xFC prefix: multi-byte opcodes
                    0xFC => {
                        let (sub_opcode, new_pos) = read_u32_leb128(body, pos);
                        encode_u32_leb128(sub_opcode, &mut out);
                        pos = new_pos;
                        match sub_opcode {
                            // memory.init: data_idx + mem_idx
                            8 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // data.drop: data_idx
                            9 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // memory.copy: mem_idx + mem_idx
                            10 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // memory.fill: mem_idx
                            11 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // table.init: elem_idx + table_idx
                            12 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // elem.drop: elem_idx
                            13 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // table.copy: table_idx + table_idx
                            14 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // table.grow/size/fill: table_idx
                            15 | 16 | 17 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // i32.trunc_sat_f32_s (0), etc. (0-7): no operands
                            0..=7 => {}
                            _ => {
                                // Unknown 0xFC sub-opcode: best-effort, no extra operands.
                            }
                        }
                    }
                    // try_table (0x06 in exception handling): blocktype + catch clauses
                    0x06 => {
                        copy_sleb128(body, &mut pos, &mut out); // blocktype
                        let (catch_count, new_pos) = read_u32_leb128(body, pos);
                        let start = pos;
                        pos = new_pos;
                        out.extend_from_slice(&body[start..pos]);
                        for _ in 0..catch_count {
                            // catch clause: catch_kind (1 byte) + optional tag_index + label
                            let catch_kind = body[pos];
                            out.push(catch_kind);
                            pos += 1;
                            match catch_kind {
                                // catch / catch_ref: tag_index + label
                                0x00 | 0x01 => {
                                    copy_uleb128(body, &mut pos, &mut out);
                                    copy_uleb128(body, &mut pos, &mut out);
                                }
                                // catch_all / catch_all_ref: label only
                                0x02 | 0x03 => {
                                    copy_uleb128(body, &mut pos, &mut out);
                                }
                                _ => {}
                            }
                        }
                    }
                    // throw (0x08): tag_index
                    0x08 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // throw_ref (0x0A): no operands
                    // All other single-byte opcodes (arithmetic, comparison,
                    // unreachable, nop, return, drop, select, etc.) have no
                    // LEB128 operands and are handled by the default copy.
                    _ => {}
                }
            }
        }
    }

    out
}

/// Copy a single unsigned LEB128 value from `src[pos..]` to `out`.
fn copy_uleb128(src: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    loop {
        let byte = src[*pos];
        out.push(byte);
        *pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
}

/// Copy a single signed LEB128 value (32-bit) from `src[pos..]` to `out`.
fn copy_sleb128(src: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    loop {
        let byte = src[*pos];
        out.push(byte);
        *pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
}

/// Copy a single signed LEB128 value (64-bit) from `src[pos..]` to `out`.
fn copy_sleb128_64(src: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    loop {
        let byte = src[*pos];
        out.push(byte);
        *pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
}

fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
) -> Vec<u8> {
    let mut func_imports: Vec<String> = Vec::new();
    let mut func_exports: BTreeMap<u32, String> = BTreeMap::new();
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
    use std::collections::BTreeSet;

    #[test]
    fn production_lir_wasm_fast_path_is_reserved_for_global_builtin_lane() {
        assert!(is_production_lir_wasm_fast_path_name(
            "molt_test____molt_globals_builtin__"
        ));
        assert!(!is_production_lir_wasm_fast_path_name(
            "molt_test_regular_helper"
        ));
        assert!(!is_production_lir_wasm_fast_path_name(
            "molt_test_user_callable"
        ));
    }

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
    fn collect_read_vars(ops: &[OpIR]) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
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

    #[test]
    fn non_linear_control_flow_detection_handles_jumpful_functions() {
        let ops = vec![
            make_op("const", None, None, Some("v0")),
            make_op("check_exception", None, None, None),
            make_op("jump", None, None, None),
            make_op("label", None, None, None),
        ];
        assert!(has_non_linear_control_flow(&ops));
    }

    #[test]
    fn non_linear_control_flow_detection_ignores_straight_line_ops() {
        let ops = vec![
            make_op("const", None, None, Some("v0")),
            make_op("add", Some(vec!["v0", "v1"]), None, Some("v2")),
            make_op("tuple_new", Some(vec!["v2"]), None, Some("v3")),
        ];
        assert!(!has_non_linear_control_flow(&ops));
    }
}
