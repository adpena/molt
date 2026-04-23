#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers

#[cfg(feature = "native-backend")]
use cranelift_codegen::Context;
#[cfg(feature = "native-backend")]
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
#[cfg(feature = "native-backend")]
use cranelift_codegen::ir::{
    AbiParam, AtomicRmwOp, Block, BlockArg, FuncRef, Function, InstBuilder, MemFlags,
    StackSlotData, StackSlotKind, Value, types,
};
#[cfg(feature = "native-backend")]
use cranelift_codegen::isa;
#[cfg(feature = "native-backend")]
use cranelift_codegen::settings;
#[cfg(feature = "native-backend")]
use cranelift_codegen::settings::Configurable;
#[cfg(feature = "native-backend")]
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
#[cfg(feature = "native-backend")]
use cranelift_module::{DataDescription, Linkage, Module};
#[cfg(feature = "native-backend")]
use cranelift_native::builder_with_options as native_isa_builder_with_options;
#[cfg(feature = "native-backend")]
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::BTreeMap;
#[cfg(feature = "native-backend")]
use std::collections::BTreeSet;
#[cfg(feature = "native-backend")]
use std::collections::HashSet;
use std::fmt::Write as _;
#[cfg(feature = "native-backend")]
use std::sync::OnceLock;

pub mod debug_artifacts;
mod ir;
mod ir_schema;
mod json_boundary;
#[cfg(feature = "llvm")]
pub mod llvm_backend;
pub mod luau_ir;
pub mod luau_lower;
#[cfg(feature = "native-backend")]
mod native_backend;
mod passes;
pub mod tir;
pub use crate::ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};
#[cfg(feature = "native-backend")]
use crate::native_backend::TrampolineKey;
pub use crate::passes::{
    apply_profile_order, build_const_int_map, elide_dead_struct_allocs,
    elide_safe_exception_checks, eliminate_dead_functions, escape_analysis, fold_constants,
    fold_constants_cross_block, hoist_loop_invariants, inline_functions, rc_coalescing,
    rewrite_stateful_loops, split_megafunctions,
};

#[cfg(feature = "luau-backend")]
pub mod luau;
#[cfg(feature = "rust-backend")]
pub mod rust;
#[cfg(feature = "wasm-backend")]
pub mod wasm;
#[cfg(feature = "wasm-backend")]
mod wasm_imports;

#[cfg(feature = "egraphs")]
pub mod egraph_simplify;

/// Pre-process phi ops into explicit store_var/load_var pairs.
///
/// The frontend emits `phi(then_val, else_val) -> out` after `end_if` to merge
/// values from if/else branches. The TIR pipeline converts structured
/// `if`/`else`/`end_if` into linearized `jump`/`label` ops but leaves `phi`
/// intact. The Cranelift backend's `phi` handler was a no-op, causing the phi
/// output to keep its entry-block None initialization.
///
/// This pass rewrites the phi pattern to explicit variable stores:
/// - In the then-branch (before `else` or `end_if`), insert `store_var out = arg0`
/// - In the else-branch (before `end_if`), insert `store_var out = arg1`
/// - Replace the `phi` op with `load_var out`
///
/// After this rewrite, the TIR SSA phase handles the merge correctly via block
/// arguments, and `lower_to_simple` emits proper `store_var`/`load_var` pairs.
pub fn rewrite_phi_to_store_load(ops: &mut Vec<OpIR>) {
    // Phase 1: find all if/else/end_if/phi patterns and collect rewrite info.
    // Build if_stack to match if/else/end_if.
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new(); // (if_idx, else_idx)
    let mut if_to_end_if: std::collections::BTreeMap<usize, usize> =
        std::collections::BTreeMap::new();
    let mut if_to_else: std::collections::BTreeMap<usize, usize> =
        std::collections::BTreeMap::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "if" => if_stack.push((idx, None)),
            "else" => {
                if let Some(top) = if_stack.last_mut() {
                    top.1 = Some(idx);
                }
            }
            "end_if" => {
                if let Some((if_idx, else_idx)) = if_stack.pop() {
                    if_to_end_if.insert(if_idx, idx);
                    if let Some(ei) = else_idx {
                        if_to_else.insert(if_idx, ei);
                    }
                }
            }
            _ => {}
        }
    }

    // Phase 2: for each end_if, scan for following phi ops and collect rewrites.
    // Rewrites: Vec<(insert_before_else_or_end_if_idx, store_var_name, store_var_arg,
    //               insert_before_end_if_idx, store_var_name2, store_var_arg2,
    //               phi_idx, phi_out)>
    struct PhiRewrite {
        then_insert_idx: usize, // index to insert store_var for then-path
        then_arg: String,       // value name for then-path
        else_insert_idx: usize, // index to insert store_var for else-path
        else_arg: String,       // value name for else-path
        phi_idx: usize,         // index of the phi op to replace
        phi_out: String,        // output variable name
    }

    let mut rewrites: Vec<PhiRewrite> = Vec::new();

    for (&if_idx, &end_if_idx) in &if_to_end_if {
        let mut scan = end_if_idx + 1;
        while scan < ops.len() && ops[scan].kind == "phi" {
            let phi_op = &ops[scan];
            if let (Some(out), Some(args)) = (&phi_op.out, &phi_op.args)
                && args.len() == 2
                && out != "none"
            {
                let has_else = if_to_else.contains_key(&if_idx);
                let then_insert;
                let else_insert;
                if has_else {
                    // Insert then-path store_var just before the `else` op.
                    then_insert = *if_to_else.get(&if_idx).unwrap();
                    // Insert else-path store_var just before end_if.
                    else_insert = end_if_idx;
                } else {
                    // No explicit else: the else-path is the fall-through
                    // from IF (condition was false). Store the else-path
                    // value BEFORE the IF so it's the default. Store the
                    // then-path value before END_IF (overrides on true).
                    else_insert = if_idx; // Before the IF
                    then_insert = end_if_idx; // Before END_IF (in then-branch)
                }
                rewrites.push(PhiRewrite {
                    then_insert_idx: then_insert,
                    then_arg: args[0].clone(),
                    else_insert_idx: else_insert,
                    else_arg: args[1].clone(),
                    phi_idx: scan,
                    phi_out: out.clone(),
                });
            }
            scan += 1;
        }
    }

    if rewrites.is_empty() {
        return;
    }

    // Phase 3: apply rewrites. Work from the end to preserve indices.
    // Sort rewrites by phi_idx descending to avoid index invalidation.
    rewrites.sort_by(|a, b| b.phi_idx.cmp(&a.phi_idx));

    for rewrite in &rewrites {
        // Replace phi with load_var.
        ops[rewrite.phi_idx] = OpIR {
            kind: "load_var".to_string(),
            var: Some(format!("_phi_{}", rewrite.phi_out)),
            out: Some(rewrite.phi_out.clone()),
            ..OpIR::default()
        };
    }

    // Now insert store_var ops. Collect all insertions, sort by index descending.
    let mut insertions: Vec<(usize, OpIR)> = Vec::new();
    for rewrite in &rewrites {
        // Store for else-path (insert before end_if).
        insertions.push((
            rewrite.else_insert_idx,
            OpIR {
                kind: "store_var".to_string(),
                var: Some(format!("_phi_{}", rewrite.phi_out)),
                args: Some(vec![rewrite.else_arg.clone()]),
                ..OpIR::default()
            },
        ));
        // Store for then-path (insert before else or end_if).
        insertions.push((
            rewrite.then_insert_idx,
            OpIR {
                kind: "store_var".to_string(),
                var: Some(format!("_phi_{}", rewrite.phi_out)),
                args: Some(vec![rewrite.then_arg.clone()]),
                ..OpIR::default()
            },
        ));
    }

    // Sort by insertion index descending to maintain correct positions.
    insertions.sort_by(|a, b| b.0.cmp(&a.0));

    for (idx, op) in insertions {
        ops.insert(idx, op);
    }
}

/// Collapse simple alias-only copy ops (`copy`, `copy_var`, `identity_alias`)
/// by rewriting later uses to the original source name.
pub fn rewrite_copy_aliases(ops: &mut [OpIR]) {
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    let resolve_alias = |name: &str, aliases: &BTreeMap<String, String>| -> String {
        let mut current = name;
        while let Some(next) = aliases.get(current) {
            current = next;
        }
        current.to_string()
    };

    for op in ops.iter_mut() {
        if let Some(var) = op.var.as_mut() {
            *var = resolve_alias(var, &aliases);
        }
        if let Some(args) = op.args.as_mut() {
            for arg in args {
                *arg = resolve_alias(arg, &aliases);
            }
        }

        match op.kind.as_str() {
            "copy_var" if op.args.is_none() => {
                if let (Some(src), Some(out)) = (op.var.as_ref(), op.out.as_ref())
                    && out != "none"
                {
                    aliases.insert(out.clone(), src.clone());
                    op.kind = "nop".to_string();
                    op.var = None;
                    op.out = None;
                }
            }
            "copy" | "identity_alias" => {
                if let (Some(args), Some(out)) = (op.args.as_ref(), op.out.as_ref())
                    && let Some(src) = args.first()
                    && out != "none"
                {
                    aliases.insert(out.clone(), src.clone());
                    op.kind = "nop".to_string();
                    op.args = None;
                    op.out = None;
                }
            }
            _ => {}
        }
    }
}

/// Replace typing-only `__annotate__` stubs with a deterministic empty-dict
/// return so all backend entrypoints preserve matching callable signatures and
/// a usable `__annotations__` value.
pub fn rewrite_annotate_stubs(ir: &mut SimpleIR) {
    for func in ir.functions.iter_mut() {
        if func.name.contains("__annotate__") {
            func.ops.clear();
            func.ops.push(OpIR {
                kind: "dict_new".to_string(),
                out: Some("__ret".to_string()),
                ..OpIR::default()
            });
            func.ops.push(OpIR {
                kind: "ret".to_string(),
                var: Some("__ret".to_string()),
                ..OpIR::default()
            });
        }
    }
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
mod native_backend_consts {
    pub(super) const QNAN: u64 = 0x7ff8_0000_0000_0000;
    pub(super) const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
    pub(super) const TAG_INT: u64 = 0x0001_0000_0000_0000;
    pub(super) const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
    pub(super) const TAG_NONE: u64 = 0x0003_0000_0000_0000;
    pub(super) const TAG_PTR: u64 = 0x0004_0000_0000_0000;
    pub(super) const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
    pub(super) const TAG_MASK: u64 = 0x0007_0000_0000_0000;
    pub(super) const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

    // ListIntStorage (#[repr(C)]) field offsets — MUST match
    // runtime/molt-runtime/src/object/layout.rs.
    // Layout: [data: *mut i64 @0, len: usize @8, cap: usize @16].
    pub(super) const LIST_INT_STORAGE_DATA_OFFSET: i32 = 0;
    pub(super) const LIST_INT_STORAGE_LEN_OFFSET: i32 = 8;
    pub(super) const INT_WIDTH: u64 = 47;
    pub(super) const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
    pub(super) const INT_SHIFT: i64 = (64 - INT_WIDTH) as i64;
    pub(super) const GENERATOR_CONTROL_BYTES: i32 = 48;
    pub(super) const TASK_KIND_FUTURE: i64 = 0;
    pub(super) const TASK_KIND_GENERATOR: i64 = 1;
    pub(super) const TASK_KIND_COROUTINE: i64 = 2;
    // FUNC_DEFAULT_* constants moved to the runtime (molt_call_func_dispatch).
    // Kept as dead_code in case the WASM backend needs them during outlining.
    #[allow(dead_code)]
    pub(super) const FUNC_DEFAULT_NONE: i64 = 1;
    #[allow(dead_code)]
    pub(super) const FUNC_DEFAULT_DICT_POP: i64 = 2;
    #[allow(dead_code)]
    pub(super) const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
    pub(super) const HEADER_SIZE_BYTES: i32 = 24;
    // MoltHeader layout (24 bytes total):
    //   offset  0: type_id    (u32)
    //   offset  4: ref_count  (u32 / AtomicU32)
    //   offset  8: flags      (u32)
    //   offset 12: size_class (u16)
    //   offset 16: cold_idx   (u32)
    //   offset 20: reserved   (u32)
    // Data pointer = header_ptr + 24, so offsets from data_ptr are negative.
    // NOTE: HEADER_STATE_OFFSET removed — state lives in cold header now;
    // the native JIT uses molt_obj_get_state/molt_obj_set_state C API calls.
    pub(super) const HEADER_REFCOUNT_OFFSET: i32 = -(HEADER_SIZE_BYTES - 4);
    pub(super) const HEADER_FLAGS_OFFSET: i32 = -(HEADER_SIZE_BYTES - 8);
    pub(super) const HEADER_FLAG_IMMORTAL: u64 = 1 << 15;
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
use native_backend_consts::*;

// ---------------------------------------------------------------------------
// Vec<u64> field layout probing.
//
// `Vec<T>` is `#[repr(Rust)]` — the compiler may reorder `{ptr, len, cap}`
// across toolchain versions (e.g. on aarch64-apple-darwin Rust 1.94 it is
// `[cap@0, ptr@8, len@16]`).  Since the Cranelift JIT emits direct memory
// loads into Vec objects owned by the runtime, we must discover the actual
// offsets at process init time.
//
// Technique: create a Vec<u64> with a unique length (7) and capacity (13),
// then scan the raw bytes of the Vec struct for those sentinel values.  The
// data pointer is the remaining 8-byte field.
// ---------------------------------------------------------------------------
#[cfg(feature = "native-backend")]
mod vec_layout {
    use std::sync::OnceLock;

    #[derive(Clone, Copy, Debug)]
    pub(crate) struct VecLayout {
        /// Offset (in bytes) of the data pointer within Vec<u64>.
        pub data_offset: i32,
        /// Offset (in bytes) of the length field within Vec<u64>.
        pub len_offset: i32,
    }

    static LAYOUT: OnceLock<VecLayout> = OnceLock::new();

    pub(crate) fn vec_u64_layout() -> VecLayout {
        *LAYOUT.get_or_init(|| {
            // Create a Vec with unique sentinel values for len and cap.
            let mut v: Vec<u64> = Vec::with_capacity(13);
            // Push exactly 7 elements so len=7, cap=13.
            for i in 0u64..7 {
                v.push(i + 0xDEAD_0000);
            }
            assert_eq!(v.len(), 7);
            assert_eq!(v.capacity(), 13);

            let vec_bytes: &[u8; std::mem::size_of::<Vec<u64>>()] =
                unsafe { &*(&v as *const Vec<u64> as *const [u8; std::mem::size_of::<Vec<u64>>()]) };

            let data_ptr = v.as_ptr() as usize;

            // Vec<u64> has exactly 3 usize-sized fields (ptr, len, cap) = 24 bytes on 64-bit.
            assert_eq!(std::mem::size_of::<Vec<u64>>(), 24);

            let mut data_offset: Option<i32> = None;
            let mut len_offset: Option<i32> = None;
            let mut cap_offset: Option<i32> = None;

            for field_idx in 0..3 {
                let byte_offset = field_idx * 8;
                let val = usize::from_ne_bytes(
                    vec_bytes[byte_offset..byte_offset + 8].try_into().unwrap(),
                );
                if val == 7 {
                    assert!(len_offset.is_none(), "Vec layout: duplicate len field");
                    len_offset = Some(byte_offset as i32);
                } else if val == 13 {
                    assert!(cap_offset.is_none(), "Vec layout: duplicate cap field");
                    cap_offset = Some(byte_offset as i32);
                } else if val == data_ptr {
                    assert!(data_offset.is_none(), "Vec layout: duplicate data field");
                    data_offset = Some(byte_offset as i32);
                } else {
                    panic!(
                        "Vec layout probe: unexpected value {val:#x} at offset {byte_offset}; \
                         expected data_ptr={data_ptr:#x}, len=7, or cap=13"
                    );
                }
            }

            let layout = VecLayout {
                data_offset: data_offset.expect("Vec layout: data pointer field not found"),
                len_offset: len_offset.expect("Vec layout: length field not found"),
            };

            // Intentionally NOT forgetting v — drop it normally.
            layout
        })
    }
}

#[cfg(feature = "native-backend")]
use vec_layout::vec_u64_layout;

/// Pre-computed NaN-box tag mask constants hoisted to the function entry block.
///
/// Cranelift can only CSE `iconst` within a single basic block.  By emitting
/// the five most-repeated tag-mask constants once in the entry block and
/// storing them in Cranelift `Variable`s, every subsequent helper call
/// (`is_int_tag`, `unbox_int`, `box_int_value`, `emit_inline_inc_ref_obj`, etc.)
/// materialises nanbox helper constants directly instead of threading them
/// through Cranelift SSA variables.
#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
struct NanBoxConsts {
    /// `(QNAN | TAG_MASK) as i64`
    qnan_tag_mask: i64,
    /// `(QNAN | TAG_INT) as i64`
    qnan_tag_int: i64,
    /// `(QNAN | TAG_PTR) as i64`
    qnan_tag_ptr: i64,
    /// `INT_SHIFT` (17)
    int_shift: i64,
    /// `POINTER_MASK as i64`
    pointer_mask: i64,
    /// `(QNAN | TAG_BOOL) as i64`
    qnan_tag_bool: i64,
    /// `INT_WIDTH as i64` (47) — used in fused_both_int_check
    int_width: i64,
    /// `48i64` — shift to isolate tag field for nanboxed-special / int checks
    shift_48: i64,
    /// `0x7FF9i64` — base of special-tag range
    special_base: i64,
    /// `5i64` — width of special-tag range
    special_limit: i64,
    /// `((QNAN | TAG_INT) >> 48) as i64` — 16-bit tag for nanboxed int check
    int_tag_16: i64,
    /// `INT_MASK as i64` — mask for box_int_value
    int_mask: i64,
    /// `16i64` — sign-extension shift for unbox_ptr_value
    shift_16: i64,
    /// `CANONICAL_NAN_BITS as i64` — canonical NaN for box_float_value
    canonical_nan: i64,
}

#[cfg(feature = "native-backend")]
impl NanBoxConsts {
    fn new(_builder: &mut FunctionBuilder) -> Self {
        Self {
            qnan_tag_mask: (QNAN | TAG_MASK) as i64,
            qnan_tag_int: (QNAN | TAG_INT) as i64,
            qnan_tag_ptr: (QNAN | TAG_PTR) as i64,
            int_shift: INT_SHIFT,
            pointer_mask: POINTER_MASK as i64,
            qnan_tag_bool: (QNAN | TAG_BOOL) as i64,
            int_width: INT_WIDTH as i64,
            shift_48: 48,
            special_base: 0x7FF9,
            special_limit: 5,
            int_tag_16: ((QNAN | TAG_INT) >> 48) as i64,
            int_mask: INT_MASK as i64,
            shift_16: 16,
            canonical_nan: CANONICAL_NAN_BITS as i64,
        }
    }
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ImportSignatureShape {
    params: Vec<String>,
    returns: Vec<String>,
}

#[cfg(feature = "native-backend")]
impl ImportSignatureShape {
    fn from_types(params: &[types::Type], returns: &[types::Type]) -> Self {
        Self {
            params: params.iter().map(ToString::to_string).collect(),
            returns: returns.iter().map(ToString::to_string).collect(),
        }
    }
}

#[cfg(feature = "native-backend")]
struct NativeBackendIrAnalysis {
    defined_functions: BTreeSet<String>,
    closure_functions: BTreeSet<String>,
    task_kinds: BTreeMap<String, TrampolineKind>,
    task_closure_sizes: BTreeMap<String, i64>,
    needs_inlining: bool,
    /// Functions that contain no user-level calls (call, call_guarded,
    /// call_func, call_internal, call_indirect, call_bind, invoke_ffi).
    /// These can skip the recursion guard on direct calls.
    leaf_functions: BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Default)]
pub struct NativeBackendModuleContext {
    function_arities: BTreeMap<String, usize>,
    function_has_ret: BTreeMap<String, bool>,
    closure_functions: BTreeSet<String>,
    task_kinds: BTreeMap<String, TrampolineKind>,
    task_closure_sizes: BTreeMap<String, i64>,
    leaf_functions: BTreeSet<String>,
    return_alias_summaries: BTreeMap<String, crate::passes::ReturnAliasSummary>,
}

#[cfg(feature = "native-backend")]
impl NativeBackendModuleContext {
    fn from_functions(functions: &[FunctionIR]) -> Self {
        let analysis = analyze_native_backend_functions(functions);
        Self {
            function_arities: functions
                .iter()
                .map(|func| (func.name.clone(), func.params.len()))
                .collect(),
            function_has_ret: compute_function_has_ret(functions),
            closure_functions: analysis.closure_functions,
            task_kinds: analysis.task_kinds,
            task_closure_sizes: analysis.task_closure_sizes,
            leaf_functions: analysis.leaf_functions,
            return_alias_summaries: crate::passes::compute_return_alias_summaries(functions),
        }
    }
}

#[cfg(feature = "native-backend")]
fn analyze_native_backend_functions(functions: &[FunctionIR]) -> NativeBackendIrAnalysis {
    let defined_functions: BTreeSet<String> = functions
        .iter()
        .filter(|func| !func.is_extern)
        .map(|func| func.name.clone())
        .collect();
    let mut closure_functions: BTreeSet<String> = BTreeSet::new();
    let mut task_kinds: BTreeMap<String, TrampolineKind> = BTreeMap::new();
    let mut task_closure_sizes: BTreeMap<String, i64> = BTreeMap::new();
    let mut needs_inlining = false;
    let mut has_task_attrs = false;

    for func_ir in functions {
        for op in &func_ir.ops {
            match op.kind.as_str() {
                "call_internal" => needs_inlining = true,
                "func_new_closure" => {
                    if let Some(name) = op.s_value.as_ref() {
                        closure_functions.insert(name.clone());
                    }
                }
                "set_attr_generic_obj" => {
                    if matches!(
                        op.s_value.as_deref(),
                        Some(
                            "__molt_is_generator__"
                                | "__molt_is_coroutine__"
                                | "__molt_is_async_generator__"
                                | "__molt_closure_size__"
                        )
                    ) {
                        has_task_attrs = true;
                    }
                }
                _ => {}
            }
        }
    }

    if has_task_attrs {
        for func_ir in functions {
            let mut func_obj_names: BTreeMap<String, String> = BTreeMap::new();
            let mut const_values: BTreeMap<String, i64> = BTreeMap::new();
            let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();
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
                        if let Some(out) = op.out.as_ref() {
                            func_obj_names.insert(out.clone(), name.clone());
                        }
                    }
                    _ => {}
                }
            }
            for op in &func_ir.ops {
                if op.kind != "set_attr_generic_obj" {
                    continue;
                }
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
                let Some(func_name) = func_obj_names.get(&args[0]) else {
                    continue;
                };
                match attr {
                    "__molt_is_generator__"
                    | "__molt_is_coroutine__"
                    | "__molt_is_async_generator__" => {
                        let val_name = &args[1];
                        let is_true = const_bools
                            .get(val_name)
                            .copied()
                            .or_else(|| const_values.get(val_name).map(|val| *val != 0))
                            .unwrap_or(false);
                        if is_true {
                            if !func_name.ends_with("_poll") {
                                continue;
                            }
                            let kind = match attr {
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
                        let val_name = &args[1];
                        if let Some(size) = const_values.get(val_name) {
                            task_closure_sizes.insert(func_name.clone(), *size);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Detect leaf functions: functions that contain no user-level calls.
    // These can safely skip the recursion guard since they cannot recurse.
    let mut leaf_functions: BTreeSet<String> = BTreeSet::new();
    for func_ir in functions {
        let has_call = func_ir.ops.iter().any(|op| {
            matches!(
                op.kind.as_str(),
                "call"
                    | "call_guarded"
                    | "call_func"
                    | "call_internal"
                    | "call_indirect"
                    | "call_bind"
                    | "invoke_ffi"
            )
        });
        if !has_call {
            leaf_functions.insert(func_ir.name.clone());
        }
    }
    if !leaf_functions.is_empty() {
        eprintln!(
            "MOLT_BACKEND: leaf functions (skip recursion guard): {} detected",
            leaf_functions.len()
        );
    }

    NativeBackendIrAnalysis {
        defined_functions,
        closure_functions,
        task_kinds,
        task_closure_sizes,
        needs_inlining,
        leaf_functions,
    }
}

#[cfg(feature = "native-backend")]
fn analyze_native_backend_ir(ir: &SimpleIR) -> NativeBackendIrAnalysis {
    analyze_native_backend_functions(&ir.functions)
}

#[cfg(feature = "native-backend")]
fn find_zero_pred_blocks(func: &Function) -> Vec<Block> {
    let mut preds: BTreeMap<Block, usize> = BTreeMap::new();
    for block in func.layout.blocks() {
        preds.entry(block).or_insert(0);
    }
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            for dest in func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
            {
                let dest_block = dest.block(&func.dfg.value_lists);
                *preds.entry(dest_block).or_insert(0) += 1;
            }
        }
    }
    let entry = func.layout.entry_block();
    preds
        .into_iter()
        .filter(|(block, count)| Some(*block) != entry && *count == 0)
        .map(|(block, _)| block)
        .collect()
}

#[cfg(feature = "native-backend")]
fn ensure_block_in_layout(builder: &mut FunctionBuilder, block: Block) {
    if builder.func.layout.is_block_inserted(block) {
        return;
    }
    if let Some(current) = builder.current_block()
        && builder.func.layout.is_block_inserted(current)
    {
        builder.insert_block_after(block, current);
        return;
    }
    builder.func.layout.append_block(block);
}

#[cfg(feature = "native-backend")]
fn block_has_terminator(builder: &FunctionBuilder, block: Block) -> bool {
    builder
        .func
        .layout
        .last_inst(block)
        .map(|inst| builder.func.dfg.insts[inst].opcode().is_terminator())
        .unwrap_or(false)
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn sync_block_filled(builder: &FunctionBuilder, is_block_filled: &mut bool) {
    if let Some(block) = builder.current_block() {
        if block_has_terminator(builder, block) {
            *is_block_filled = true;
        } else {
            // The current block is open (no terminator) — clear the flag so
            // subsequent ops are not incorrectly skipped.  This fixes cases
            // where a control-flow op (e.g. check_exception) switched to a
            // fresh fallthrough block and cleared the flag via
            // switch_to_block_tracking, but a stale `true` value from a
            // previous iteration leaked through.
            *is_block_filled = false;
        }
    }
}

#[cfg(feature = "native-backend")]
fn switch_to_block_tracking(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    // Guard: if the block already has a terminator instruction, Cranelift's
    // `switch_to_block` will panic with "you cannot switch to a block which
    // is already filled".  This happens in complex control flow (e.g. stdlib
    // modules with nested try/except + if/else) where multiple paths converge
    // on the same block and a previous path already sealed it with a branch.
    // In that case we must NOT switch to it — just mark as filled so
    // subsequent ops create a fresh block or skip dead code.
    if block_has_terminator(builder, block) {
        *is_block_filled = true;
        return;
    }
    ensure_block_in_layout(builder, block);
    builder.switch_to_block(block);
    *is_block_filled = false;
}

#[cfg(feature = "native-backend")]
fn resolve_cleanup_value(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    entry_vars: &BTreeMap<String, Value>,
    name: &str,
) -> Option<Value> {
    entry_vars
        .get(name)
        .copied()
        .or_else(|| var_get(builder, vars, name).map(|v| *v))
}

#[cfg(feature = "native-backend")]
fn box_int(val: i64) -> i64 {
    // Use INT_MASK (47 bits) not POINTER_MASK (48 bits) to match the
    // sign-extending unbox path (ishl/sshr by INT_SHIFT=17).
    let masked = (val as u64) & INT_MASK;
    (QNAN | TAG_INT | masked) as i64
}

#[cfg(feature = "native-backend")]
fn box_float(val: f64) -> i64 {
    if val.is_nan() {
        // Canonicalize NaN to avoid collision with the QNAN tag prefix.
        // Must match CANONICAL_NAN_BITS in molt-obj-model.
        0x7ff0_0000_0000_0001_u64 as i64
    } else {
        val.to_bits() as i64
    }
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
fn pending_bits() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

#[cfg(feature = "native-backend")]
fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

#[cfg(feature = "native-backend")]
fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
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
    // Keep the id within inline-int payload range and avoid zero.
    let id = (hash & ((1u64 << 46) - 1)).max(1);
    id as i64
}

#[cfg(feature = "native-backend")]
fn unbox_int(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    // Debug-mode guard: verify the value actually carries the int tag before
    // unboxing.  In release builds this is a no-op; in debug builds an illegal
    // trap fires immediately if a non-int value reaches this path.
    #[cfg(debug_assertions)]
    {
        let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
        let expected = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
        let masked = builder.ins().band(val, mask);
        let is_int = builder.ins().icmp(IntCC::Equal, masked, expected);
        builder
            .ins()
            .trapz(is_int, cranelift_codegen::ir::TrapCode::user(1).unwrap());
    }

    // The ishl by INT_SHIFT (17) shifts out the upper 17 tag bits (QNAN+TAG),
    // then sshr sign-extends the 47-bit payload. No separate band with INT_MASK
    // is needed — the shift pair implicitly strips the tag.
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(val, shift);
    builder.ins().sshr(shifted, shift)
}

/// Unbox a NaN-boxed value that is either TAG_INT or TAG_BOOL to an i64.
///
/// Booleans are coerced to 0/1 (matching Python's `bool` subclass of `int`).
/// This is needed in `fast_int` arithmetic paths where the TIR optimizer may
/// mark an op as `fast_int` even when one or both operands are booleans.
#[cfg(feature = "native-backend")]
fn unbox_int_or_bool(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    let masked = builder.ins().band(val, mask);
    let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);

    let bool_block = builder.create_block();
    let int_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);

    builder.ins().brif(is_bool, bool_block, &[], int_block, &[]);

    // Bool path: extract bit 0 as the integer value (False=0, True=1).
    builder.switch_to_block(bool_block);
    builder.seal_block(bool_block);
    let one = builder.ins().iconst(types::I64, 1);
    let bool_val = builder.ins().band(val, one);
    jump_block(builder, merge_block, &[bool_val]);

    // Int path: normal unbox_int shift pair.
    builder.switch_to_block(int_block);
    builder.seal_block(int_block);
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(val, shift);
    let int_val = builder.ins().sshr(shifted, shift);
    jump_block(builder, merge_block, &[int_val]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    builder.block_params(merge_block)[0]
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
fn is_int_tag(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let masked = builder.ins().band(val, mask);
    builder.ins().icmp(IntCC::Equal, masked, tag)
}

/// Fused tag-check-and-unbox for a single NaN-boxed value.
///
/// XORs the value against the expected int tag pattern `(QNAN | TAG_INT)`.
/// If the value is an int, the XOR zeros out the upper 17 tag bits, leaving
/// only the 47-bit payload.
///
/// Returns `(xored, unboxed)` where:
///   - `xored` can be used for the tag check: `(xored >> 47) == 0` iff the
///     value was a NaN-boxed int.
///   - `unboxed` is the sign-extended 47-bit integer payload (valid only when
///     the tag check passes).
#[cfg(feature = "native-backend")]
fn fused_tag_check_and_unbox_int(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> (Value, Value) {
    let expected_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let xored = builder.ins().bxor(val, expected_tag);
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(xored, shift);
    let unboxed = builder.ins().sshr(shifted, shift);
    (xored, unboxed)
}

/// Check that two XOR'd values both represent NaN-boxed ints.
///
/// Takes the `xored` outputs from two `fused_tag_check_and_unbox_int` calls
/// and checks that both had their tag bits zeroed (i.e., both were ints).
/// Uses BOR to combine the two values, then checks that the upper 17 bits
/// of the combined result are zero — true iff both inputs were ints.
#[cfg(feature = "native-backend")]
fn fused_both_int_check(
    builder: &mut FunctionBuilder,
    lhs_xored: Value,
    rhs_xored: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let combined = builder.ins().bor(lhs_xored, rhs_xored);
    let tag_shift = builder.ins().iconst(types::I64, nbc.int_width);
    let upper = builder.ins().ushr(combined, tag_shift);
    builder.ins().icmp_imm(IntCC::Equal, upper, 0)
}

/// Check whether a NaN-boxed value is a special tagged type (int/bool/none/ptr/pending)
/// rather than a plain f64.
///
/// All NaN-boxed specials have bits 62..48 in the range `0x7FF9..=0x7FFD`.
/// Returns true if the value IS a special (i.e., NOT a float).
#[cfg(feature = "native-backend")]
fn is_nanboxed_special(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    // Shift right by 48 to isolate the tag field, then check range [0x7FF9, 0x7FFD].
    let shift48 = builder.ins().iconst(types::I64, nbc.shift_48);
    let tag16 = builder.ins().ushr(val, shift48);
    // tag16 - 0x7FF9; result < 5 means it's a tagged special
    let base = builder.ins().iconst(types::I64, nbc.special_base);
    let adjusted = builder.ins().isub(tag16, base);
    let limit = builder.ins().iconst(types::I64, nbc.special_limit);
    builder.ins().icmp(IntCC::UnsignedLessThan, adjusted, limit)
}

/// Check that both NaN-boxed values are plain f64 (not tagged specials).
#[cfg(feature = "native-backend")]
fn both_float_check(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let lhs_special = is_nanboxed_special(builder, lhs, nbc);
    let rhs_special = is_nanboxed_special(builder, rhs, nbc);
    let either_special = builder.ins().bor(lhs_special, rhs_special);
    // both_float = !(lhs_special || rhs_special)
    // Since is_nanboxed_special returns an i8 (0 or 1), we check either_special == 0
    builder.ins().icmp_imm(IntCC::Equal, either_special, 0)
}

/// Check whether a NaN-boxed value carries the int tag.
#[cfg(feature = "native-backend")]
fn is_nanboxed_int(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let shift48 = builder.ins().iconst(types::I64, nbc.shift_48);
    let tag16 = builder.ins().ushr(val, shift48);
    let expected = builder.ins().iconst(types::I64, nbc.int_tag_16);
    builder.ins().icmp(IntCC::Equal, tag16, expected)
}

/// Emit inline mixed int+float arithmetic.  When exactly one operand is a
/// NaN-boxed int and the other is a plain f64, convert the int to f64 via
/// `fcvt_from_sint` and perform the requested float operation inline.
///
/// `f_op`: 0 = fadd, 1 = fsub, 2 = fmul.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_mixed_int_float_op(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
    nbc: &NanBoxConsts,
    f_op: u8,
    merge_block: Block,
) {
    let lhs_is_int = is_nanboxed_int(builder, lhs, nbc);
    let rhs_is_int = is_nanboxed_int(builder, rhs, nbc);
    let lhs_special = is_nanboxed_special(builder, lhs, nbc);
    let rhs_special = is_nanboxed_special(builder, rhs, nbc);
    let rhs_not_special = builder.ins().icmp_imm(IntCC::Equal, rhs_special, 0);
    let lhs_not_special = builder.ins().icmp_imm(IntCC::Equal, lhs_special, 0);
    let case_a = builder.ins().band(lhs_is_int, rhs_not_special);
    let case_b = builder.ins().band(rhs_is_int, lhs_not_special);
    let lhs_int_block = builder.create_block();
    let check_rhs_block = builder.create_block();
    let rhs_int_block = builder.create_block();
    let not_mixed_block = builder.create_block();
    builder.set_cold_block(not_mixed_block);
    builder
        .ins()
        .brif(case_a, lhs_int_block, &[], check_rhs_block, &[]);
    // LHS is int, RHS is float
    builder.switch_to_block(lhs_int_block);
    builder.seal_block(lhs_int_block);
    let lhs_int_val = unbox_int(builder, lhs, nbc);
    let lhs_conv = builder.ins().fcvt_from_sint(types::F64, lhs_int_val);
    let rhs_flt = builder.ins().bitcast(types::F64, MemFlags::new(), rhs);
    let res_a = match f_op {
        0 => builder.ins().fadd(lhs_conv, rhs_flt),
        1 => builder.ins().fsub(lhs_conv, rhs_flt),
        2 => builder.ins().fmul(lhs_conv, rhs_flt),
        _ => unreachable!(),
    };
    let boxed_a = box_float_value(builder, res_a, nbc);
    jump_block(builder, merge_block, &[boxed_a]);
    // Check case_b
    builder.switch_to_block(check_rhs_block);
    builder.seal_block(check_rhs_block);
    builder
        .ins()
        .brif(case_b, rhs_int_block, &[], not_mixed_block, &[]);
    // RHS is int, LHS is float
    builder.switch_to_block(rhs_int_block);
    builder.seal_block(rhs_int_block);
    let rhs_int_val = unbox_int(builder, rhs, nbc);
    let rhs_conv = builder.ins().fcvt_from_sint(types::F64, rhs_int_val);
    let lhs_flt = builder.ins().bitcast(types::F64, MemFlags::new(), lhs);
    let res_b = match f_op {
        0 => builder.ins().fadd(lhs_flt, rhs_conv),
        1 => builder.ins().fsub(lhs_flt, rhs_conv),
        2 => builder.ins().fmul(lhs_flt, rhs_conv),
        _ => unreachable!(),
    };
    let boxed_b = box_float_value(builder, res_b, nbc);
    jump_block(builder, merge_block, &[boxed_b]);
    // Not mixed: caller emits slow path
    builder.switch_to_block(not_mixed_block);
    builder.seal_block(not_mixed_block);
}

#[cfg(feature = "native-backend")]
fn box_int_value(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.int_mask);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    builder.ins().bor(tag, masked)
}

#[cfg(feature = "native-backend")]
fn box_float_value(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    // Canonicalize NaN: if the f64 value is NaN, replace with CANONICAL_NAN_BITS
    // to avoid collision with the QNAN tag prefix used by NaN-boxing.
    let raw_bits = builder.ins().bitcast(types::I64, MemFlags::new(), val);
    let is_nan = builder.ins().fcmp(FloatCC::Unordered, val, val);
    let canonical = builder.ins().iconst(types::I64, nbc.canonical_nan);
    builder.ins().select(is_nan, canonical, raw_bits)
}

#[cfg(feature = "native-backend")]
fn int_value_fits_inline(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Inline ints are 47-bit signed payloads: range [-(1<<46), (1<<46)-1].
    // Bias the value by +2^46 so the valid range maps to [0, 2^47-1],
    // then do a single unsigned comparison against 2^47.
    // This is a single-comparison range check that Cranelift cannot fold away.
    let bias = builder.ins().iconst(types::I64, 1_i64 << 46);
    let biased = builder.ins().iadd(val, bias);
    let limit = builder.ins().iconst(types::I64, 1_i64 << 47);
    builder.ins().icmp(IntCC::UnsignedLessThan, biased, limit)
}

/// Perform `imul` with 64-bit overflow detection via `smulhi`.
///
/// Two 47-bit signed values can produce a product exceeding 64 bits (up to ~93
/// bits).  Plain `imul` silently wraps at 64 bits, and the truncated result may
/// happen to pass `int_value_fits_inline` even though it is wrong.
///
/// Returns `(product, fits)` where `product` is the low 64 bits of the
/// multiplication and `fits` is a boolean Value that is true only when:
///   1. The full 128-bit product equals the 64-bit `imul` result (no 64-bit
///      overflow), AND
///   2. The 64-bit result fits in a 47-bit signed inline integer.
#[cfg(feature = "native-backend")]
fn imul_checked_inline(builder: &mut FunctionBuilder, lhs: Value, rhs: Value) -> (Value, Value) {
    let prod = builder.ins().imul(lhs, rhs);
    // smulhi gives the upper 64 bits of the signed 128-bit product.
    let hi = builder.ins().smulhi(lhs, rhs);
    // If there was no 64-bit overflow, hi must be the sign-extension of prod,
    // i.e. hi == prod >> 63 (arithmetic).
    let sixty_three = builder.ins().iconst(types::I64, 63);
    let sign = builder.ins().sshr(prod, sixty_three);
    let no_overflow_64 = builder.ins().icmp(IntCC::Equal, hi, sign);
    // Also check the result fits in 47-bit signed payload.
    let fits_47 = int_value_fits_inline(builder, prod);
    let both_ok = builder.ins().band(no_overflow_64, fits_47);
    (prod, both_ok)
}

#[cfg(feature = "native-backend")]
fn box_bool_value(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    let bool_val = builder.ins().select(val, one, zero);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    builder.ins().bor(tag, bool_val)
}

#[cfg(feature = "native-backend")]
fn unbox_ptr_value(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.pointer_mask);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, nbc.shift_16);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
}

#[cfg(feature = "native-backend")]
fn box_ptr_value(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.pointer_mask);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    builder.ins().bor(tag, masked)
}

/// Fully inline list_int bounds check — zero FFI calls.
///
/// Extracts the raw heap pointer from the NaN-boxed list value, then
/// dereferences the object layout directly:
///
///   obj_ptr  = unbox_ptr(list_bits)   // past MoltHeader
///   vec_ptr  = *(obj_ptr as *const *const Vec<i64>)   // offset 0
///   data_ptr = *(vec_ptr + 0)         // Vec::ptr  (offset 0)
///   len      = *(vec_ptr + 8)         // Vec::len  (offset 8)
///
/// Returns (data_ptr, in_bounds) — the caller must branch on in_bounds
/// BEFORE loading/storing the element.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_list_int_bounds_check(
    builder: &mut FunctionBuilder,
    list_bits: Value,
    index_raw: Value,
    _nbc: &NanBoxConsts,
) -> (Value, Value) {
    // Step 1: extract raw pointer from NaN-boxed value.
    //
    // The NaN-boxed pointer layout is: QNAN | TAG_PTR | (addr & POINTER_MASK).
    // To extract the address: mask off the top 16 bits (QNAN+tag), then
    // sign-extend from bit 47 to reconstruct canonical aarch64 addresses.
    //
    // Use _imm variants to avoid introducing SSA variable dependencies that
    // could interact with Cranelift's block sealing in complex control flow.
    let masked = builder.ins().band_imm(list_bits, POINTER_MASK as i64);
    // Sign-extend from bit 47: shift left 16, arithmetic shift right 16.
    let shifted = builder.ins().ishl_imm(masked, 16);
    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
    // Step 2: load *mut Vec<i64> from offset 0 of the object payload
    let vec_ptr = builder
        .ins()
        .load(types::I64, MemFlags::trusted(), obj_ptr, 0);
    // Step 3: load data pointer from Vec (offset 0) and length (offset 8)
    let data_ptr = builder
        .ins()
        .load(types::I64, MemFlags::trusted(), vec_ptr, 0);
    let len = builder
        .ins()
        .load(types::I64, MemFlags::trusted(), vec_ptr, 8);
    // Step 4: unsigned compare index < length
    let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index_raw, len);
    (data_ptr, in_bounds)
}

/// Load element from list_int data pointer at given index.
/// MUST only be called after bounds check passes (i.e., inside the fast block).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_list_int_load(
    builder: &mut FunctionBuilder,
    data_ptr: Value,
    index_raw: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let offset = builder.ins().imul_imm(index_raw, 8);
    let elem_addr = builder.ins().iadd(data_ptr, offset);
    let raw_val = builder
        .ins()
        .load(types::I64, MemFlags::trusted(), elem_addr, 0);
    box_int_value(builder, raw_val, nbc)
}

/// Store element into list_int data pointer at given index.
/// MUST only be called after bounds check passes (i.e., inside the fast block).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_list_int_store(
    builder: &mut FunctionBuilder,
    data_ptr: Value,
    index_raw: Value,
    value_raw: Value,
) {
    let offset = builder.ins().imul_imm(index_raw, 8);
    let elem_addr = builder.ins().iadd(data_ptr, offset);
    builder
        .ins()
        .store(MemFlags::trusted(), value_raw, elem_addr, 0);
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
fn emit_maybe_ref_adjust(builder: &mut FunctionBuilder, val: Value, obj_ref_fn: FuncRef) {
    // Keep ref-adjust control flow linear. Hidden branch blocks here can invalidate
    // block-local tracked-value carry if callers do not explicitly propagate tracking.
    // The runtime ref helpers already no-op for non-pointer boxed values.
    let _ = builder.ins().call(obj_ref_fn, &[val]);
}

// ---------------------------------------------------------------------------
// Phase 1: Inline inc_ref_obj as Cranelift IR
//
// Eliminates function-call overhead for the hottest runtime operation (~73
// calls per compiled function). The inlined sequence:
//
//   1. Check if `val` is a heap pointer (NaN-boxed TAG_PTR).
//   2. Extract the raw data pointer from the NaN-box.
//   3. Load the flags field from MoltHeader; skip if IMMORTAL.
//   4. Load the 32-bit refcount, add 1, store back.
//
// Gated by MOLT_INLINE_RC=1 env var so we can A/B test vs call-based RC.
// dec_ref is left as a function call (needs the free/destructor path).
// ---------------------------------------------------------------------------

/// Returns `true` if inline RC codegen is enabled.
///
/// Re-enabled: the inline RC path now uses atomic_rmw (AtomicRmwOp::Add)
/// instead of non-atomic load/iadd/store, which is correct for the
/// AtomicU32 refcount field.
#[cfg(feature = "native-backend")]
fn inline_rc_enabled() -> bool {
    // Disabled: inline RC codegen (even single-branch) causes memory corruption
    // when inc_ref blocks fragment the control flow inside tuple_new. The root
    // cause is Cranelift's handling of SSA values across the brif boundary
    // between the inc_ref blocks and subsequent list_builder_append calls.
    // The function-call path (molt_inc_ref_obj) is both correct and fast
    // enough — it matches Swift's ARC pattern of opaque retain/release calls.
    false
}

/// Emit an inlined `inc_ref_obj` as Cranelift IR.
///
/// Single-branch architecture: only one brif (is_ptr → inc, else → merge).
/// The immortal check uses branchless conditional select to compute the
/// increment delta (0 for immortal, 1 for mortal), avoiding the extra
/// block that caused the Cranelift block-fragmentation corruption bug.
///
/// Equivalent to:
/// ```text
/// if (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR):
///     ptr = sign_extend(val & POINTER_MASK)
///     flags = *(ptr - 8) as u32
///     delta = ((flags & IMMORTAL) == 0) ? 1 : 0
///     atomic_add(*(ptr - 12), delta)  // no-op when delta=0
/// ```
#[cfg(feature = "native-backend")]
fn emit_inline_inc_ref_obj(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) {
    // Single-branch: only split on is_ptr to avoid block fragmentation.
    let inc_block = builder.create_block();
    let merge_block = builder.create_block();

    // 1. Check if val is a heap pointer: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let tag_check_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    builder.ins().brif(is_ptr, inc_block, &[], merge_block, &[]);

    // 2. Inc block: extract pointer, check immortal branchlessly, atomic inc
    builder.switch_to_block(inc_block);
    let raw_ptr = unbox_ptr_value(builder, val, nbc);

    // Load flags and compute delta branchlessly:
    // delta = (flags & IMMORTAL) == 0 ? 1 : 0
    let flags = builder.ins().load(
        types::I32,
        MemFlags::trusted(),
        raw_ptr,
        HEADER_FLAGS_OFFSET,
    );
    let immortal_mask = builder
        .ins()
        .iconst(types::I32, HEADER_FLAG_IMMORTAL as i64);
    let immortal_bits = builder.ins().band(flags, immortal_mask);
    let zero_i32 = builder.ins().iconst(types::I32, 0);
    let is_mortal = builder.ins().icmp(IntCC::Equal, immortal_bits, zero_i32);
    let one_i32 = builder.ins().iconst(types::I32, 1);
    // Branchless: delta = select(is_mortal, 1, 0)
    let delta = builder.ins().select(is_mortal, one_i32, zero_i32);

    // Atomic add of delta (0 for immortal = no-op, 1 for mortal = inc)
    let rc_offset = builder
        .ins()
        .iconst(types::I64, HEADER_REFCOUNT_OFFSET as i64);
    let rc_addr = builder.ins().iadd(raw_ptr, rc_offset);
    builder.ins().atomic_rmw(
        types::I32,
        MemFlags::trusted(),
        AtomicRmwOp::Add,
        rc_addr,
        delta,
    );
    builder.ins().jump(merge_block, &[]);

    // 3. Merge
    builder.switch_to_block(merge_block);
    builder.seal_block(inc_block);
    builder.seal_block(merge_block);
}

/// Emit an inc_ref_obj — either inlined or as a function call depending on
/// the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
fn emit_inc_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val, nbc);
    } else {
        builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a ref-adjust (inc_ref_obj) — either inlined or as a function call
/// depending on the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
fn emit_maybe_ref_adjust_v2(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val, nbc);
    } else {
        let _ = builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a dec_ref_obj with an inlined tag check: if the value is not a heap
/// pointer (e.g. NaN-boxed int/float/bool/none), skip the dec_ref call
/// entirely. This eliminates function-call + GIL overhead for the common case
/// where cleanup values are immediate integers.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_dec_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if !inline_rc_enabled() {
        builder.ins().call(call_ref, &[val]);
        return;
    }
    // Inline tag check: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let call_block = builder.create_block();
    let merge_block = builder.create_block();

    let tag_check_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    brif_block(builder, is_ptr, call_block, &[], merge_block, &[]);

    // Only call dec_ref_obj for actual heap pointers.
    builder.switch_to_block(call_block);
    builder.ins().call(call_ref, &[val]);
    jump_block(builder, merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(call_block);
    builder.seal_block(merge_block);
}

#[derive(Clone, Copy)]
#[cfg(feature = "native-backend")]
struct VarValue(Value);

#[cfg(feature = "native-backend")]
impl std::ops::Deref for VarValue {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

#[cfg(feature = "native-backend")]
fn var_get(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: &str,
) -> Option<VarValue> {
    vars.get(name).map(|var| VarValue(builder.use_var(*var)))
}

#[cfg(feature = "native-backend")]
fn def_var_named(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: impl AsRef<str>,
    val: Value,
) {
    let name_ref = name.as_ref();
    if name_ref == "none" {
        return;
    }
    let var = *vars
        .get(name_ref)
        .unwrap_or_else(|| panic!("Var not found: {name_ref}"));
    builder.def_var(var, val);
}

/// Seal a block only if it hasn't been sealed yet. Prevents the
/// `!self.is_sealed(block)` assertion panic in Cranelift's SSA builder
/// when multiple code paths attempt to seal the same block.
#[cfg(feature = "native-backend")]
#[inline]
fn seal_block_once(
    builder: &mut FunctionBuilder,
    sealed: &mut std::collections::BTreeSet<Block>,
    block: Block,
) {
    if sealed.insert(block) && builder.func.layout.is_block_inserted(block) {
        builder.seal_block(block);
    }
}

#[cfg(feature = "native-backend")]
fn jump_block(builder: &mut FunctionBuilder, target: Block, args: &[Value]) {
    let block_args: Vec<BlockArg> = args.iter().copied().map(BlockArg::from).collect();
    builder.ins().jump(target, &block_args);
}

#[cfg(feature = "native-backend")]
fn brif_block(
    builder: &mut FunctionBuilder,
    cond: Value,
    then_block: Block,
    then_args: &[Value],
    else_block: Block,
    else_args: &[Value],
) {
    let then_block_args: Vec<BlockArg> = then_args.iter().copied().map(BlockArg::from).collect();
    let else_block_args: Vec<BlockArg> = else_args.iter().copied().map(BlockArg::from).collect();
    builder.ins().brif(
        cond,
        then_block,
        &then_block_args,
        else_block,
        &else_block_args,
    );
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn parse_inst_id(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(b"inst") {
            let mut j = i + 4;
            let mut value: usize = 0;
            let mut found = false;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                found = true;
                value = value * 10 + (bytes[j] - b'0') as usize;
                j += 1;
            }
            if found {
                return Some(value);
            }
        }
        i += 1;
    }
    None
}

struct DumpIrConfig {
    mode: String,
    filter: Option<String>,
}

pub(crate) fn should_dump_ir() -> Option<DumpIrConfig> {
    let raw = std::env::var("MOLT_DUMP_IR").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let (mode, filter) = if let Some((left, right)) = trimmed.split_once(':') {
        let left_trim = left.trim();
        let right_trim = right.trim();
        let mode = if left_trim.eq_ignore_ascii_case("full") {
            "full"
        } else {
            "control"
        };
        let filter = if right_trim.is_empty() {
            None
        } else {
            Some(right_trim.to_string())
        };
        (mode.to_string(), filter)
    } else if lower == "full" || lower == "control" || lower == "1" || lower == "all" {
        let mode = if lower == "full" { "full" } else { "control" };
        (mode.to_string(), None)
    } else {
        ("control".to_string(), Some(trimmed.to_string()))
    };
    Some(DumpIrConfig { mode, filter })
}

pub(crate) fn dump_ir_matches(config: &DumpIrConfig, func_name: &str) -> bool {
    let Some(filter) = config.filter.as_ref() else {
        return true;
    };
    if filter == "1" || filter.eq_ignore_ascii_case("all") {
        return true;
    }
    func_name == filter || func_name.contains(filter)
}

#[cfg(feature = "native-backend")]
struct TraceOpsConfig {
    stride: usize,
}

#[cfg(feature = "native-backend")]
fn should_trace_ops(func_name: &str) -> Option<TraceOpsConfig> {
    static RAW: OnceLock<Option<String>> = OnceLock::new();
    let raw = RAW
        .get_or_init(|| {
            std::env::var("MOLT_TRACE_OP_PROGRESS")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .as_ref()?;
    let (filter_part, stride_part) = match raw.split_once(':') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (raw.as_str(), None),
    };
    let stride = stride_part
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5_000);
    let matches = filter_part == "1"
        || filter_part.eq_ignore_ascii_case("all")
        || func_name == filter_part
        || func_name.contains(filter_part);
    if matches {
        Some(TraceOpsConfig { stride })
    } else {
        None
    }
}

pub(crate) fn dump_ir_ops(func_ir: &FunctionIR, mode: &str) {
    let mut out = String::new();
    let full = mode.eq_ignore_ascii_case("full");
    let mut last_written = 0usize;
    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !full {
            let kind = op.kind.as_str();
            let is_control = matches!(
                kind,
                "if" | "else"
                    | "end_if"
                    | "phi"
                    | "label"
                    | "state_label"
                    | "jump"
                    | "br_if"
                    | "loop_start"
                    | "loop_end"
                    | "loop_break_if_true"
                    | "loop_break_if_false"
                    | "loop_break"
                    | "loop_continue"
                    | "ret"
            );
            if !is_control {
                continue;
            }
        }
        let mut detail = Vec::new();
        if let Some(out_name) = &op.out {
            detail.push(format!("out={out_name}"));
        }
        if let Some(var) = &op.var {
            detail.push(format!("var={var}"));
        }
        if let Some(args) = &op.args {
            detail.push(format!("args=[{}]", args.join(", ")));
        }
        if let Some(val) = op.value {
            detail.push(format!("value={val}"));
        }
        if let Some(val) = op.f_value {
            detail.push(format!("f_value={val}"));
        }
        if let Some(val) = &op.s_value {
            detail.push(format!("s_value={val}"));
        }
        if let Some(bytes) = &op.bytes {
            detail.push(format!("bytes_len={}", bytes.len()));
        }
        if let Some(fast_int) = op.fast_int {
            detail.push(format!("fast_int={fast_int}"));
        }
        let _ = writeln!(out, "{idx:04}: {:<20} {}", op.kind, detail.join(" "));
        last_written = idx;
    }
    if last_written == 0 && func_ir.ops.is_empty() {
        return;
    }
    eprintln!("IR ops for {} (mode={}):\n{}", func_ir.name, mode, out);
    if std::env::var("MOLT_DUMP_IR_FILE").as_deref() == Ok("1") {
        let _ = std::fs::create_dir_all("logs");
        let sanitized = func_ir
            .name
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
                _ => '_',
            })
            .collect::<String>();
        let path = std::path::Path::new("logs").join(format!("ir_dump_{sanitized}.log"));
        let _ = std::fs::write(path, &out);
    }
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn collect_cleanup_tracked(
    names: &[String],
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<String> {
    names
        .iter()
        .filter(|name| skip != Some(name.as_str()))
        .filter(|name| last_use.get(*name).copied().unwrap_or(op_idx) <= op_idx)
        .cloned()
        .collect()
}

#[cfg(feature = "native-backend")]
fn extend_unique_tracked(dst: &mut Vec<String>, src: Vec<String>) {
    if src.is_empty() {
        return;
    }
    if dst.is_empty() {
        dst.extend(src);
        return;
    }
    // Dedup by `name` so multi-predecessor merges don't create double-decref hazards.
    let mut seen: BTreeSet<String> = dst.iter().cloned().collect();
    for name in src {
        if seen.insert(name.clone()) {
            dst.push(name);
        }
    }
}

/// Propagate tracked objects to ALL branch target blocks.
/// Prevents use-after-free when exception handlers access freed objects.
#[cfg(feature = "native-backend")]
pub(crate) fn propagate_tracked_to_branches(
    block_tracked: &mut BTreeMap<cranelift_codegen::ir::Block, Vec<String>>,
    targets: &[cranelift_codegen::ir::Block],
    carry: Vec<String>,
) {
    if carry.is_empty() || targets.is_empty() {
        return;
    }
    if targets.len() == 1 {
        extend_unique_tracked(block_tracked.entry(targets[0]).or_default(), carry);
        return;
    }
    let last_idx = targets.len() - 1;
    for (i, &target) in targets.iter().enumerate() {
        if i == last_idx {
            extend_unique_tracked(block_tracked.entry(target).or_default(), carry);
            return;
        }
        extend_unique_tracked(block_tracked.entry(target).or_default(), carry.clone());
    }
}

#[cfg(feature = "native-backend")]
fn drain_cleanup_tracked_dedup(
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    mut already_decrefed: Option<&mut BTreeSet<String>>,
) -> Vec<String> {
    let mut cleanup = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        let cleanup_key = alias_roots
            .get(name)
            .map(String::as_str)
            .unwrap_or(name.as_str());
        if let Some(ref set) = already_decrefed
            && set.contains(cleanup_key)
        {
            return false;
        }
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            if let Some(ref mut set) = already_decrefed {
                set.insert(cleanup_key.to_string());
            }
            cleanup.push(name.clone());
            return false;
        }
        true
    });
    cleanup
}

#[cfg(feature = "native-backend")]
fn drain_cleanup_entry_tracked(
    names: &mut Vec<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<Value> {
    let mut cleanup = Vec::new();
    let mut to_remove = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        // If not in last_use, default to MAX (keep alive) — NOT op_idx.
        // Using op_idx as default causes premature cleanup of variables
        // that are used later but not yet tracked in last_use.
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            let cleanup_key = alias_roots
                .get(name)
                .map(String::as_str)
                .unwrap_or(name.as_str());
            if already_decrefed.contains(cleanup_key) {
                to_remove.push(name.clone());
                return false;
            }
            if let Some(val) = entry_vars.get(name) {
                cleanup.push(*val);
            }
            already_decrefed.insert(cleanup_key.to_string());
            // Mark for removal from entry_vars so no other cleanup path
            // (exception handler, finalize block) can double dec-ref.
            to_remove.push(name.clone());
            return false;
        }
        true
    });
    for name in to_remove {
        entry_vars.remove(&name);
    }
    cleanup
}

// ---------------------------------------------------------------------------
// RC coalescing: eliminate redundant inc_ref / dec_ref pairs.
// ---------------------------------------------------------------------------

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
const CONTROL_FLOW_OPS: &[&str] = &[
    "if",
    "else",
    "end_if",
    "loop_start",
    "loop_end",
    "loop_for_start",
    "loop_for_end",
    "label",
    "state_label",
    "jump",
    "return",
    "state_yield",
    "check_exception",
    "raise",
];

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    let cf_set: HashSet<&str> = CONTROL_FLOW_OPS.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::new();
    let mut skip_dec_ref: HashSet<String> = HashSet::new();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) {
            continue;
        }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind.as_str(), "inc_ref" | "borrow");
        let a_is_dec = matches!(a.kind.as_str(), "dec_ref" | "release");
        if !a_is_inc && !a_is_dec {
            continue;
        }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(b.kind.as_str()) {
                break;
            }
            let b_kind = b.kind.as_str();
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, "dec_ref" | "release") && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, "inc_ref" | "borrow") && b_arg.map(String::as_str) == Some(&a_arg)
            };
            if is_match && !skip_ops.contains(&j) {
                skip_ops.insert(i);
                skip_ops.insert(j);
                break;
            }
            let uses_var = b
                .args
                .as_ref()
                .map(|args| args.iter().any(|n| n == &a_arg))
                .unwrap_or(false)
                || b.var.as_ref().map(|v| v == &a_arg).unwrap_or(false)
                || b.out.as_ref().map(|o| o == &a_arg).unwrap_or(false);
            if uses_var {
                break;
            }
        }
    }

    for (idx, op) in ops.iter().enumerate() {
        if skip_ops.contains(&idx) {
            continue;
        }
        if !matches!(op.kind.as_str(), "inc_ref" | "borrow") {
            continue;
        }
        let out_name = match op.out.as_deref() {
            Some(name) if name != "none" => name,
            _ => continue,
        };
        let last = last_use.get(out_name).copied().unwrap_or(idx);
        if last <= idx {
            skip_ops.insert(idx);
            skip_dec_ref.insert(out_name.to_string());
        }
    }

    if !skip_ops.is_empty() || !skip_dec_ref.is_empty() {
        static RC_COALESCE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *RC_COALESCE_TRACE
            .get_or_init(|| std::env::var("MOLT_RC_COALESCE_TRACE").as_deref() == Ok("1"));
        if trace {
            eprintln!(
                "[rc-coalesce] eliminated {} RC ops, {} dec_ref skips",
                skip_ops.len(),
                skip_dec_ref.len()
            );
        }
    }

    (skip_ops, skip_dec_ref)
}

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) enum TrampolineKind {
    Plain,
    Generator,
    Coroutine,
    AsyncGen,
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct TrampolineSpec {
    pub(crate) arity: usize,
    pub(crate) has_closure: bool,
    pub(crate) kind: TrampolineKind,
    pub(crate) closure_size: i64,
    /// Whether the target function returns a value. Trampolines use this
    /// to set the correct import signature — functions with ret_void only
    /// don't have a return in their signature.
    #[cfg_attr(
        not(any(feature = "native-backend", feature = "llvm")),
        allow(dead_code)
    )]
    pub(crate) target_has_ret: bool,
}

#[cfg(feature = "native-backend")]
pub struct SimpleBackend {
    module: ObjectModule,
    ctx: Context,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    trampoline_ids: BTreeMap<TrampolineKey, cranelift_module::FuncId>,
    import_ids: BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    pub skip_ir_passes: bool,
    pub skip_shared_stdlib_partition: bool,
    /// Function names that exist in other batches — use Linkage::Import, not trap stubs.
    pub external_function_names: std::collections::BTreeSet<String>,
    module_context: Option<NativeBackendModuleContext>,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    data_pool: BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: u64,
    // Track the arity each user-defined function was declared with so that
    // call sites that reference the same function (potentially with a
    // different number of actual arguments, e.g. kwargs expansion) can
    // construct a matching Cranelift signature for `declare_function`.
    declared_func_arities: BTreeMap<String, usize>,
    /// Track which functions have been given a body (defined), so we can
    /// emit trap stubs for declared-but-undefined `__ov` variants after
    /// all functions are compiled.
    defined_func_names: std::collections::BTreeSet<String>,
    /// Deferred Cranelift function definitions for parallel compilation.
    /// Instead of compiling each function immediately in `define_function`,
    /// we collect the finalized IR here and compile them all in parallel
    /// via `flush_deferred_defines()`.
    deferred_defines: Vec<DeferredDefine>,
}

#[cfg(feature = "native-backend")]
pub(crate) struct DeferredDefine {
    pub(crate) func_id: cranelift_module::FuncId,
    pub(crate) func: cranelift_codegen::ir::Function,
    pub(crate) name: String,
}

#[cfg(feature = "native-backend")]
struct IfFrame {
    else_block: Option<Block>,
    merge_block: Block,
    has_else: bool,
    then_terminal: bool,
    else_terminal: bool,
    phi_ops: Vec<(String, String, String)>,
    phi_params: Vec<Value>,
    merge_rebind_names: Vec<String>,
    merge_rebind_params: Vec<Value>,
    merge_rebind_slots: Vec<cranelift_codegen::ir::StackSlot>,
}

#[cfg(feature = "native-backend")]
struct LoopFrame {
    loop_block: Block,
    body_block: Block,
    after_block: Block,
    index_name: Option<String>,
    next_index: Option<Value>,
    next_index_raw: Option<Value>,
    /// True when the loop uses the linearized TIR path (no dedicated
    /// Cranelift loop block; counter flows through phi variables).
    /// `loop_end` must NOT decrement `loop_depth` for linearized loops
    /// because `loop_index_start` did not increment it.
    linearized: bool,
}

#[cfg(feature = "native-backend")]
fn parse_truthy_env(raw: &str) -> bool {
    let norm = raw.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(feature = "native-backend")]
fn compute_function_has_ret(functions: &[FunctionIR]) -> BTreeMap<String, bool> {
    functions
        .iter()
        .map(|func| {
            let has_ret = func.ops.iter().any(|op| op.kind == "ret");
            (func.name.clone(), has_ret)
        })
        .collect()
}

#[cfg(feature = "native-backend")]
fn merge_function_arities(
    module_context: Option<&NativeBackendModuleContext>,
    local_function_arities: BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let mut merged = module_context
        .map(|context| context.function_arities.clone())
        .unwrap_or_default();
    merged.extend(local_function_arities);
    merged
}

#[cfg(feature = "native-backend")]
fn merge_function_has_ret(
    module_context: Option<&NativeBackendModuleContext>,
    local_function_has_ret: BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let mut merged = module_context
        .map(|context| context.function_has_ret.clone())
        .unwrap_or_default();
    merged.extend(local_function_has_ret);
    merged
}

pub(crate) fn env_setting(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

#[cfg(feature = "native-backend")]
fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

#[cfg(feature = "native-backend")]
fn emitted_name_matches_module_symbol(name: &str, module_symbol: &str) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

#[cfg(feature = "native-backend")]
fn explicit_stdlib_module_symbols_from_env() -> Option<BTreeSet<String>> {
    let raw = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok()?;
    let parsed: Vec<String> = serde_json::from_str(&raw).ok()?;
    Some(parsed.into_iter().collect())
}

#[cfg(feature = "native-backend")]
fn is_user_owned_symbol(
    name: &str,
    entry_module: &str,
    stdlib_module_symbols: Option<&BTreeSet<String>>,
) -> bool {
    let entry_init = format!("molt_init_{entry_module}");
    if name == "molt_main"
        || name.starts_with(&format!("{entry_module}__"))
        || name == entry_init
        || name == "molt_init___main__"
        || name == "molt_isolate_import"
        || name == "molt_isolate_bootstrap"
    {
        return true;
    }
    if let Some(stdlib_module_symbols) = stdlib_module_symbols {
        if let Some(module_symbol) = emitted_module_symbol(name) {
            return !stdlib_module_symbols.contains(module_symbol);
        }
        return !stdlib_module_symbols
            .iter()
            .any(|module_symbol| emitted_name_matches_module_symbol(name, module_symbol));
    }
    false
}

#[cfg(feature = "native-backend")]
fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&BTreeSet<String>>,
) -> (Vec<FunctionIR>, Vec<FunctionIR>) {
    eliminate_dead_functions(ir);
    let user_func_set: BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| is_user_owned_symbol(&f.name, entry_module, stdlib_module_symbols))
        .map(|f| f.name.clone())
        .collect();
    let all_funcs: Vec<_> = ir.functions.drain(..).collect();
    let (user_remaining, mut stdlib_funcs): (Vec<_>, Vec<_>) = all_funcs
        .into_iter()
        .partition(|f| user_func_set.contains(&f.name));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    stdlib_funcs.retain(|f| seen.insert(f.name.clone()));
    (user_remaining, stdlib_funcs)
}

#[cfg(feature = "native-backend")]
fn externalize_shared_stdlib_partition(ir: &mut SimpleIR) {
    let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
        return;
    };
    let Ok(entry_module) = std::env::var("MOLT_ENTRY_MODULE") else {
        return;
    };
    let stdlib_path = std::path::Path::new(&stdlib_obj_path);
    if !stdlib_path.exists() {
        return;
    }
    let explicit_stdlib_module_symbols = explicit_stdlib_module_symbols_from_env();
    let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
        ir,
        &entry_module,
        explicit_stdlib_module_symbols.as_ref(),
    );
    let mut retained = std::mem::take(&mut user_remaining);
    for mut func in std::mem::take(&mut stdlib_funcs) {
        func.is_extern = true;
        func.ops.clear();
        retained.push(func);
    }
    ir.functions = retained;
}

#[cfg(feature = "native-backend")]
impl Default for SimpleBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    fn cloned_shared_flags(
        flags: &settings::Flags,
        opt_level_override: Option<&str>,
    ) -> Result<settings::Flags, String> {
        let mut builder = settings::builder();
        for value in flags.iter() {
            let configured = if value.name == "opt_level" {
                opt_level_override
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.value_string())
            } else {
                value.value_string()
            };
            builder
                .set(value.name, &configured)
                .map_err(|err| format!("shared flag {}={configured:?}: {err}", value.name))?;
        }
        Ok(settings::Flags::new(builder))
    }

    fn rebuild_owned_isa(
        target_isa: &dyn isa::TargetIsa,
        opt_level_override: Option<&str>,
    ) -> Result<isa::OwnedTargetIsa, String> {
        let isa_builder = isa::Builder::from_target_isa(target_isa);
        let shared_flags = Self::cloned_shared_flags(target_isa.flags(), opt_level_override)?;
        isa_builder
            .finish(shared_flags)
            .map_err(|err| format!("TargetIsa finish: {err}"))
    }

    pub fn new() -> Self {
        Self::new_with_target(None)
    }

    pub fn new_with_target(target: Option<&str>) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        // Cranelift optimization level: "none", "speed", or "speed_and_size".
        // Default to "speed" for production quality codegen.  Override with
        // MOLT_BACKEND_OPT_LEVEL=none for fast dev-loop compilation (~3-5x
        // faster compile times at the cost of ~30-50% slower generated code).
        let opt_level =
            env_setting("MOLT_BACKEND_OPT_LEVEL").unwrap_or_else(|| "speed".to_string());
        flag_builder
            .set("opt_level", &opt_level)
            .unwrap_or_else(|err| panic!("invalid MOLT_BACKEND_OPT_LEVEL={opt_level:?}: {err:?}"));
        let regalloc_algorithm =
            env_setting("MOLT_BACKEND_REGALLOC_ALGORITHM").unwrap_or_else(|| {
                // When opt_level=none, default to the fast single-pass
                // allocator regardless of build profile — the user has
                // explicitly asked for compile-time speed.
                if opt_level == "none" {
                    "single_pass".to_string()
                } else {
                    "backtracking".to_string()
                }
            });
        flag_builder
            .set("regalloc_algorithm", &regalloc_algorithm)
            .unwrap_or_else(|err| {
                panic!("invalid MOLT_BACKEND_REGALLOC_ALGORITHM={regalloc_algorithm:?}: {err:?}")
            });
        // Cranelift 0.128 adds explicit minimum function alignment tuning.
        // Default to 16-byte release alignment for better i-cache/branch
        // behavior on hot call-heavy kernels; keep debug/dev unchanged.
        let min_alignment_log2 = env_setting("MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2")
            .unwrap_or_else(|| {
                if cfg!(debug_assertions) {
                    "0".to_string()
                } else {
                    "4".to_string()
                }
            });
        flag_builder
            .set("log2_min_function_alignment", &min_alignment_log2)
            .unwrap_or_else(|err| {
                panic!(
                    "invalid MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2={min_alignment_log2:?}: {err:?}"
                )
            });
        if let Some(libcall_call_conv) = env_setting("MOLT_BACKEND_LIBCALL_CALL_CONV") {
            flag_builder
                .set("libcall_call_conv", &libcall_call_conv)
                .unwrap_or_else(|err| {
                    panic!("invalid MOLT_BACKEND_LIBCALL_CALL_CONV={libcall_call_conv:?}: {err:?}")
                });
        }
        // Cranelift verifier catches IR invariant violations (type mismatches,
        // dominator tree bugs). Enable in debug builds; disable in release for
        // speed. Override with MOLT_BACKEND_ENABLE_VERIFIER=0|1.
        let default_enable_verifier = cfg!(debug_assertions);
        let enable_verifier = env_setting("MOLT_BACKEND_ENABLE_VERIFIER")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(default_enable_verifier);
        flag_builder
            .set(
                "enable_verifier",
                if enable_verifier { "true" } else { "false" },
            )
            .unwrap();
        // Cranelift alias analysis: enables redundant-load elimination across
        // memory operations within a basic block. Safe for our codegen because
        // we never emit raw pointer aliasing between different object fields.
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        // Emit CFG metadata in machine code output — enables downstream tools
        // and profilers to reconstruct control-flow graphs from compiled objects.
        flag_builder.set("machine_code_cfg_info", "true").unwrap();
        // Use colocated libcalls: our generated code and runtime libcalls live
        // in the same link unit — colocated calls skip GOT/PLT indirection and
        // use direct PC-relative calls instead.
        flag_builder.set("use_colocated_libcalls", "true").unwrap();
        // Detect whether we are targeting aarch64 — either because we are
        // compiling natively on aarch64, or because an explicit cross-compile
        // target triple was supplied that contains "aarch64".
        let targeting_aarch64 = match target {
            Some(t) => t.contains("aarch64"),
            None => cfg!(target_arch = "aarch64"),
        };
        // Frame pointers: always preserve on aarch64 to ensure correct stack
        // frame layout for large functions (>16KB frames).  Cranelift 0.128 can
        // generate incorrect SP-relative accesses on aarch64 when frame pointers
        // are omitted and the frame exceeds the immediate offset range, leading
        // to SIGTRAP (exit 133) in generated code.  On x86_64 the cost is one
        // register (rbp); on aarch64 x29 is conventionally reserved anyway.
        // Debug builds always preserve for profiler/debugger support.
        flag_builder
            .set(
                "preserve_frame_pointers",
                if cfg!(debug_assertions) || targeting_aarch64 {
                    "true"
                } else {
                    "false"
                },
            )
            .unwrap();
        // Spectre mitigations: Molt compiles trusted user code (not sandboxed
        // plugins), so Spectre v1 heap/table mitigations add unnecessary overhead.
        flag_builder
            .set("enable_heap_access_spectre_mitigation", "false")
            .unwrap();
        flag_builder
            .set("enable_table_access_spectre_mitigation", "false")
            .unwrap();
        // Stack probing strategy: use outline (call-based) probes on aarch64
        // to avoid a Cranelift 0.128 bug where inline probe loops generate
        // incorrect touch sequences for frames >16KB, causing SIGTRAP.
        // On x86_64, inline probes are safe and faster for deep recursion.
        flag_builder
            .set(
                "probestack_strategy",
                if targeting_aarch64 {
                    "outline"
                } else {
                    "inline"
                },
            )
            .unwrap();
        // MOLT_PORTABLE=1 forces baseline ISA (no host-specific features like AVX2).
        // This ensures reproducible codegen across different machines at the cost of
        // ~5-15% runtime performance on modern CPUs with advanced features.
        let portable = env_setting("MOLT_PORTABLE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let mut isa_builder = if let Some(triple) = target {
            isa::lookup_by_name(triple).unwrap_or_else(|msg| {
                panic!("target {} is not supported: {}", triple, msg);
            })
        } else if portable {
            // Baseline ISA: no auto-detected host features. Produces portable
            // binaries that run on any CPU supporting the base architecture.
            native_isa_builder_with_options(false).unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        } else {
            // Auto-detect host CPU features (AVX2, SSE4.2, BMI2, POPCNT on x86;
            // NEON, AES, CRC on aarch64). Allows Cranelift to emit feature-specific
            // instructions like vpmovmskb, popcnt, tzcnt, etc.
            native_isa_builder_with_options(true).unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        };

        // Ensure critical ISA-specific features are explicitly enabled when the
        // CPU supports them. While native_isa_builder_with_options(true) probes
        // CPUID/system registers, explicit enablement here serves as a safety net
        // for edge cases (custom target triples, future Cranelift changes) and
        // documents our performance-critical feature requirements.
        //
        // x86_64: BMI1/BMI2 (tzcnt, blsr for bit manipulation in hash probing),
        //         POPCNT (popcount for set operations and hash table occupancy).
        // aarch64: LSE (atomic CAS/SWP for lock-free refcount operations).
        #[cfg(target_arch = "x86_64")]
        if !portable && target.is_none() {
            if std::arch::is_x86_feature_detected!("bmi1") {
                let _ = isa_builder.enable("has_bmi1");
            }
            if std::arch::is_x86_feature_detected!("bmi2") {
                let _ = isa_builder.enable("has_bmi2");
            }
            if std::arch::is_x86_feature_detected!("popcnt") {
                let _ = isa_builder.enable("has_popcnt");
            }
        }
        #[cfg(target_arch = "aarch64")]
        if !portable && target.is_none() && std::arch::is_aarch64_feature_detected!("lse") {
            let _ = isa_builder.enable("has_lse");
        }

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let mut builder = ObjectBuilder::new(
            isa,
            "molt_output",
            cranelift_module::default_libcall_names(),
        )
        .unwrap();
        // Emit each function into its own object section so the linker can
        // discard unreferenced runtime functions via -dead_strip / --gc-sections.
        builder.per_function_section(true);
        let module = ObjectModule::new(builder);
        let ctx = module.make_context();

        Self {
            module,
            ctx,
            trampoline_ids: BTreeMap::new(),
            import_ids: BTreeMap::new(),
            skip_ir_passes: false,
            skip_shared_stdlib_partition: false,
            external_function_names: std::collections::BTreeSet::new(),
            module_context: None,
            data_pool: BTreeMap::new(),
            next_data_id: 0,
            declared_func_arities: BTreeMap::new(),
            defined_func_names: std::collections::BTreeSet::new(),
            deferred_defines: Vec::new(),
        }
    }

    pub fn build_module_context(functions: &[FunctionIR]) -> NativeBackendModuleContext {
        NativeBackendModuleContext::from_functions(functions)
    }

    pub fn set_module_context(&mut self, context: NativeBackendModuleContext) {
        self.module_context = Some(context);
    }

    /// Retry compiling a function at `opt_level=none` after the optimizing
    /// pipeline panicked.  Builds a throwaway ISA that matches the module's
    /// target but disables all optimization passes (which avoids the
    /// `remove_constant_phis` assertion and similar upstream Cranelift bugs).
    /// The compiled bytes are installed via `define_function_bytes` so the
    /// module's own ISA is never consulted for code generation.
    fn retry_define_at_opt_none(
        module: &mut ObjectModule,
        func_id: cranelift_module::FuncId,
        func: cranelift_codegen::ir::Function,
        func_name: &str,
    ) -> Result<(), String> {
        use cranelift_codegen::control::ControlPlane;

        let fallback_isa = Self::rebuild_owned_isa(module.isa(), Some("none"))?;

        let mut retry_ctx = Context::for_function(func);
        let mut ctrl = ControlPlane::default();
        retry_ctx
            .compile(&*fallback_isa, &mut ctrl)
            .map_err(|e| format!("compile at O0: {e:?}"))?;
        let compiled = retry_ctx.compiled_code().unwrap();
        let alignment = compiled.buffer.alignment as u64;
        let code = compiled.buffer.data().to_vec();
        let relocs: Vec<cranelift_module::ModuleReloc> = compiled
            .buffer
            .relocs()
            .iter()
            .map(|r| cranelift_module::ModuleReloc::from_mach_reloc(r, &retry_ctx.func, func_id))
            .collect();
        module
            .define_function_bytes(func_id, alignment, &code, &relocs)
            .map_err(|e| format!("define_function_bytes for {func_name}: {e}"))?;
        Ok(())
    }

    /// Compile all deferred function definitions in parallel using rayon,
    /// then define the resulting bytes sequentially via `define_function_bytes`.
    /// Functions that panic during optimized compilation are retried at
    /// `opt_level=none`; if that also fails, a trap stub is emitted.
    fn flush_deferred_defines(&mut self) {
        use cranelift_codegen::control::ControlPlane;
        use rayon::prelude::*;

        let deferred: Vec<DeferredDefine> = std::mem::take(&mut self.deferred_defines);
        if deferred.is_empty() {
            return;
        }

        // Compile all functions in parallel. Each worker gets its own
        // Context + ControlPlane but shares one rebuilt OwnedTargetIsa.
        struct CompiledFunc {
            func_id: cranelift_module::FuncId,
            name: String,
            alignment: u64,
            code: Vec<u8>,
            relocs: Vec<cranelift_module::ModuleReloc>,
        }
        enum CompileResult {
            Ok(CompiledFunc),
            /// Optimizing compilation panicked or errored -- carry the function
            /// IR for a sequential retry at opt_level=none.
            NeedsRetry {
                func_id: cranelift_module::FuncId,
                func: Box<cranelift_codegen::ir::Function>,
                name: String,
            },
        }

        let compile_isa = Self::rebuild_owned_isa(self.module.isa(), None)
            .unwrap_or_else(|err| panic!("failed to rebuild TargetIsa for deferred flush: {err}"));

        let results: Vec<CompileResult> = {
            // Arc<dyn TargetIsa> contains a raw pointer that isn't marked
            // Send/Sync, but the target ISA is immutable after construction and
            // safe to share across parallel Cranelift compilation workers.
            #[derive(Clone)]
            struct SendIsa(std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>);
            unsafe impl Send for SendIsa {}
            unsafe impl Sync for SendIsa {}

            let compile_isa = SendIsa(compile_isa);
            let mut indexed: Vec<(usize, CompileResult)> = deferred
                .into_par_iter()
                .enumerate()
                .map(|(idx, item)| {
                    let isa = compile_isa.clone().0;
                    let mut ctx = Context::for_function(item.func);
                    let mut ctrl = ControlPlane::default();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        ctx.compile(&*isa, &mut ctrl)
                            .map(|_| ())
                            .map_err(|e| format!("{e:?}"))
                    }));
                    let compile_result = match result {
                        Ok(Ok(())) => {
                            let compiled = ctx.compiled_code().unwrap();
                            let alignment = compiled.buffer.alignment as u64;
                            let code = compiled.buffer.data().to_vec();
                            let relocs: Vec<cranelift_module::ModuleReloc> = compiled
                                .buffer
                                .relocs()
                                .iter()
                                .map(|r| {
                                    cranelift_module::ModuleReloc::from_mach_reloc(
                                        r,
                                        &ctx.func,
                                        item.func_id,
                                    )
                                })
                                .collect();
                            CompileResult::Ok(CompiledFunc {
                                func_id: item.func_id,
                                name: item.name,
                                alignment,
                                code,
                                relocs,
                            })
                        }
                        Ok(Err(err)) => {
                            eprintln!(
                                "WARNING: Cranelift compilation error in `{}`; will retry: {err}",
                                item.name
                            );
                            CompileResult::NeedsRetry {
                                func_id: item.func_id,
                                func: Box::new(ctx.func),
                                name: item.name,
                            }
                        }
                        Err(_panic) => {
                            eprintln!(
                                "WARNING: Cranelift optimizer panic in `{}`; will retry at opt_level=none",
                                item.name
                            );
                            CompileResult::NeedsRetry {
                                func_id: item.func_id,
                                func: Box::new(ctx.func),
                                name: item.name,
                            }
                        }
                    };
                    (idx, compile_result)
                })
                .collect();
            indexed.sort_by_key(|(idx, _)| *idx);
            indexed.into_iter().map(|(_, result)| result).collect()
        };

        // Sequential phase: define compiled functions and handle retries.
        for result in results {
            match result {
                CompileResult::Ok(cf) => {
                    if let Err(e) = self.module.define_function_bytes(
                        cf.func_id,
                        cf.alignment,
                        &cf.code,
                        &cf.relocs,
                    ) {
                        eprintln!("ERROR: define_function_bytes failed for {}: {e}", cf.name);
                    } else {
                        self.defined_func_names.insert(cf.name);
                    }
                }
                CompileResult::NeedsRetry {
                    func_id,
                    func,
                    name,
                } => {
                    // Wrap retry in catch_unwind: Cranelift can panic
                    // even at opt_level=none (e.g. blockorder or
                    // alias_analysis on functions with orphaned blocks).
                    let retry_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            Self::retry_define_at_opt_none(&mut self.module, func_id, *func, &name)
                        }));
                    match retry_result {
                        Ok(Ok(())) => {
                            self.defined_func_names.insert(name.clone());
                            eprintln!("  -> {} compiled successfully at opt_level=none", name);
                        }
                        Ok(Err(retry_err)) => {
                            eprintln!("  -> retry also failed for {}: {}", name, retry_err);
                            let sig = self
                                .module
                                .declarations()
                                .get_function_decl(func_id)
                                .signature
                                .clone();
                            eprintln!("  -> emitting trap stub for {} (Cranelift error)", name);
                            match Self::emit_trap_stub(&mut self.module, func_id, &sig, &name) {
                                Ok(()) => {
                                    self.defined_func_names.insert(name);
                                }
                                Err(stub_err) => {
                                    eprintln!(
                                        "  -> trap stub also failed for {}: {}",
                                        name, stub_err
                                    );
                                }
                            }
                        }
                        Err(_panic) => {
                            eprintln!("  -> retry panicked for {}", name);
                            let sig = self
                                .module
                                .declarations()
                                .get_function_decl(func_id)
                                .signature
                                .clone();
                            eprintln!("  -> emitting trap stub for {} (Cranelift panic)", name);
                            match Self::emit_trap_stub(&mut self.module, func_id, &sig, &name) {
                                Ok(()) => {
                                    self.defined_func_names.insert(name);
                                }
                                Err(stub_err) => {
                                    eprintln!(
                                        "  -> trap stub also failed for {}: {}",
                                        name, stub_err
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Emit a minimal function body that immediately traps.  Used as a
    /// fallback when a function is too large for Cranelift to compile
    /// (even at opt_level=none).  The stub lets the rest of the object
    /// file link successfully; if the function is called at runtime,
    /// it will abort.
    fn emit_trap_stub(
        module: &mut ObjectModule,
        func_id: cranelift_module::FuncId,
        sig: &cranelift_codegen::ir::Signature,
        func_name: &str,
    ) -> Result<(), String> {
        use cranelift_codegen::control::ControlPlane;
        use cranelift_codegen::ir::{Function, TrapCode};
        use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

        let mut func = Function::with_name_signature(
            cranelift_codegen::ir::UserFuncName::default(),
            sig.clone(),
        );
        let mut fbc = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut fbc);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_all_blocks();
        builder.ins().trap(TrapCode::user(1).unwrap());
        builder.finalize();

        let fallback_isa = Self::rebuild_owned_isa(module.isa(), Some("none"))?;

        let mut ctx = Context::for_function(func);
        let mut ctrl = ControlPlane::default();
        ctx.compile(&*fallback_isa, &mut ctrl)
            .map_err(|e| format!("compile trap stub: {e:?}"))?;
        let compiled = ctx.compiled_code().unwrap();
        let alignment = compiled.buffer.alignment as u64;
        let code = compiled.buffer.data().to_vec();
        let relocs: Vec<cranelift_module::ModuleReloc> = compiled
            .buffer
            .relocs()
            .iter()
            .map(|r| cranelift_module::ModuleReloc::from_mach_reloc(r, &ctx.func, func_id))
            .collect();
        module
            .define_function_bytes(func_id, alignment, &code, &relocs)
            .map_err(|e| format!("define_function_bytes trap stub for {func_name}: {e}"))?;
        Ok(())
    }

    fn intern_data_segment(
        module: &mut ObjectModule,
        data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
        next_data_id: &mut u64,
        bytes: &[u8],
    ) -> cranelift_module::DataId {
        if let Some(existing) = data_pool.get(bytes) {
            return *existing;
        }
        let name = format!("data_pool_{}", *next_data_id);
        *next_data_id += 1;
        let data_id = module
            .declare_data(&name, Linkage::Local, false, false)
            .unwrap();
        let mut data_ctx = DataDescription::new();
        data_ctx.define(bytes.to_vec().into_boxed_slice());
        module.define_data(data_id, &data_ctx).unwrap();
        data_pool.insert(bytes.to_vec(), data_id);
        data_id
    }

    /// Walk backwards from `before_idx` to find a `"const"` op whose `out`
    /// matches `var_name` and return its integer value.  Used by the
    /// iter_next peephole to resolve constant index arguments.
    fn resolve_const_int(ops: &[OpIR], before_idx: usize, var_name: &str) -> Option<i64> {
        for i in (0..before_idx).rev() {
            let op = &ops[i];
            if op.kind == "const"
                && let Some(ref out) = op.out
                && out == var_name
            {
                return op.value;
            }
        }
        None
    }

    /// Cached version of `module.declare_function(name, Linkage::Import, &sig)`.
    /// Returns the `FuncId` for the given runtime import, reusing a previous
    /// declaration when the same name has already been declared.  The signature
    /// shape is validated on cache hits to guard against mismatches.
    ///
    /// Takes split borrows (`module` + `import_ids`) so callers can hold a
    /// concurrent `FunctionBuilder` borrow on `self.ctx.func`.
    fn import_func_id_split(
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        name: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> cranelift_module::FuncId {
        let shape = ImportSignatureShape::from_types(params, returns);
        if let Some((func_id, cached_shape)) = import_ids.get(name) {
            assert_eq!(
                cached_shape, &shape,
                "import signature mismatch for {name}: {:?} vs {:?}",
                cached_shape, shape
            );
            return *func_id;
        }

        let mut sig = module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(*param));
        }
        for ret in returns {
            sig.returns.push(AbiParam::new(*ret));
        }
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap();
        import_ids.insert(name, (func_id, shape));
        func_id
    }

    /// Convenience wrapper around `import_func_id_split` for use when
    /// `&mut self` is not split-borrowed (e.g. in tests).
    #[cfg(test)]
    fn import_func_id(
        &mut self,
        name: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> cranelift_module::FuncId {
        Self::import_func_id_split(
            &mut self.module,
            &mut self.import_ids,
            name,
            params,
            returns,
        )
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        let timing = env_setting("MOLT_BACKEND_TIMING")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let compile_start = std::time::Instant::now();
        let mut ir = ir;
        // Backend selection: MOLT_BACKEND=llvm routes through LLVM when the feature is
        // available; otherwise falls back to Cranelift with a warning.
        let _use_llvm = env_setting("MOLT_BACKEND").as_deref() == Some("llvm");
        #[cfg(not(feature = "llvm"))]
        if _use_llvm {
            eprintln!(
                "[molt] WARNING: MOLT_BACKEND=llvm requested but llvm feature is not compiled in; falling back to Cranelift"
            );
        }
        apply_profile_order(&mut ir);
        // ── Pre-TIR IR passes (parallel) ────────────────────────
        // Each pass operates on a single FunctionIR with no shared
        // mutable state, so all 8 passes can run in parallel across
        // functions using rayon.  Fusing them into one par_iter_mut
        // avoids 8× thread-pool dispatch overhead and improves cache
        // locality (each function stays hot while all passes run).
        {
            use rayon::prelude::*;
            let disable_rc = std::env::var("MOLT_DISABLE_RC_COALESCING").as_deref() == Ok("1");
            ir.functions.par_iter_mut().for_each(|func_ir| {
                rewrite_stateful_loops(func_ir);
                elide_dead_struct_allocs(func_ir);
                escape_analysis(func_ir);
                if !disable_rc {
                    rc_coalescing(func_ir);
                }
                fold_constants(&mut func_ir.ops);
                fold_constants_cross_block(&mut func_ir.ops);
                elide_safe_exception_checks(func_ir);
                hoist_loop_invariants(func_ir);
            });
        }
        // ── GPU kernel detection ──
        // Functions containing GPU intrinsic ops (gpu_thread_id, gpu_block_id,
        // etc.) are GPU kernels.  Flag them in metadata so the GPU pipeline can
        // handle them separately, but they still flow through the canonical
        // TIR/LIR pipeline like every other function.
        let mut gpu_kernel_names: Vec<String> = Vec::new();
        for func_ir in &ir.functions {
            let is_gpu = func_ir.ops.iter().any(|op| {
                matches!(
                    op.kind.as_str(),
                    "gpu_thread_id"
                        | "gpu_block_id"
                        | "gpu_block_dim"
                        | "gpu_grid_dim"
                        | "gpu_barrier"
                )
            });
            if is_gpu {
                gpu_kernel_names.push(func_ir.name.clone());
            }
        }
        if !gpu_kernel_names.is_empty() {
            eprintln!(
                "[molt-gpu] Detected {} GPU kernel function(s): {:?}",
                gpu_kernel_names.len(),
                gpu_kernel_names
            );
        }

        // ── TIR optimization pipeline (default ON; set MOLT_TIR_OPT=0 to disable) ──
        // The TIR roundtrip (lower->refine->optimize->lower-back) is enabled by
        // default.  Functions that crash Cranelift compilation get a trap stub
        // via the catch_unwind retry path in flush_deferred_defines.
        //
        // All TIR-lowered control flow uses pure label/jump/br_if patterns
        // (no structured loop_start/loop_end).  The Cranelift function compiler
        // handles back-edges via has_loop_or_backedge detection.
        let mut tir_optimized_names: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        // TIR default ON: loop markers preserved, EH functions bypassed.
        if env_setting("MOLT_TIR_OPT").as_deref() != Some("0") {
            use rayon::prelude::*;

            let _tir_dump = env_setting("TIR_DUMP").as_deref() == Some("1");
            let _tir_stats = env_setting("TIR_OPT_STATS").as_deref() == Some("1");
            let mut tir_cache =
                crate::tir::cache::CompilationCache::open(crate::tir::cache::backend_cache_dir());

            // Phase 1 (sequential): check cache for every function. For cache
            // hits, apply immediately. For misses, collect the function index
            // and content hash.
            struct TirWorkItem {
                index: usize,
                content_hash: String,
            }
            let mut work_items: Vec<TirWorkItem> = Vec::new();

            // Debug: dump raw IR for functions matching MOLT_DUMP_FUNC_IR pattern.
            let dump_func_pattern = std::env::var("MOLT_DUMP_FUNC_IR").ok();

            for (i, func_ir) in ir.functions.iter_mut().enumerate() {
                // Extern functions: bodies live in stdlib_shared.o.
                // They are registered as external before codegen.
                if func_ir.is_extern {
                    continue;
                }

                // Dump raw ops to file for debugging TIR roundtrip issues.
                if let Some(ref pattern) = dump_func_pattern
                    && func_ir.name.contains(pattern.as_str())
                {
                    let sanitized: String = func_ir
                        .name
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let mut dump = String::new();
                    dump.push_str(&format!(
                        "// func: {} ({} ops)\n",
                        func_ir.name,
                        func_ir.ops.len()
                    ));
                    dump.push_str(&format!("// params: {:?}\n", func_ir.params));
                    dump.push_str(&format!("// param_types: {:?}\n", func_ir.param_types));
                    for (idx, op) in func_ir.ops.iter().enumerate() {
                        dump.push_str(&format!("{:4}: kind={:30} out={:20} var={:20} args={:40} val={:?} sval={:?} fi={:?} ff={:?}\n",
                                idx, op.kind,
                                op.out.as_deref().unwrap_or(""),
                                op.var.as_deref().unwrap_or(""),
                                op.args.as_ref().map(|a| a.join(",")).unwrap_or_default(),
                                op.value, op.s_value, op.fast_int, op.fast_float));
                    }
                    let _ = crate::debug_artifacts::write_debug_artifact(
                        format!("ir/{sanitized}.txt"),
                        dump,
                    );
                }

                // Loop markers (loop_start, loop_end) are now preserved through
                // the TIR roundtrip via LoopRole metadata on TirFunction, so
                // functions with loops benefit from TIR optimization.
                let body_bytes = crate::tir::serialize::serialize_ops(&func_ir.ops);
                let content_hash = crate::tir::cache::CompilationCache::compute_hash_with_signature(
                    &func_ir.name,
                    &func_ir.params,
                    func_ir.param_types.as_deref(),
                    &body_bytes,
                );
                // Check TIR cache: if we have validated optimized ops from a
                // previous build with the same content hash, reuse them.
                if let Some(cached_bytes) = tir_cache.get(&content_hash)
                    && let Some(cached_ops) = crate::tir::serialize::deserialize_ops(&cached_bytes)
                {
                    func_ir.ops = cached_ops;
                    tir_optimized_names.insert(func_ir.name.clone());
                    continue;
                }
                work_items.push(TirWorkItem {
                    index: i,
                    content_hash,
                });
            }

            let uncached_count = work_items.len();
            if uncached_count > 0 {
                eprintln!(
                    "MOLT_BACKEND: TIR optimizing {uncached_count} uncached functions in parallel"
                );
                let tir_start = std::time::Instant::now();

                // Phase 2 (parallel): run the TIR pipeline on every uncached
                // function.  Each work item borrows only its own FunctionIR
                // (via index) and produces an independent result.
                //
                // We cannot borrow &mut ir.functions[i] in parallel because
                // Rust's borrow checker does not allow multiple mutable refs
                // into the same Vec, even at disjoint indices, through closures.
                // Instead we extract the ops, optimize them in parallel, and
                // write them back.
                struct TirInput {
                    index: usize,
                    content_hash: String,
                    name: String,
                    params: Vec<String>,
                    ops: Vec<OpIR>,
                    param_types: Option<Vec<String>>,
                }
                let inputs: Vec<TirInput> = work_items
                    .into_iter()
                    .map(|wi| {
                        let func_ir = &ir.functions[wi.index];
                        TirInput {
                            index: wi.index,
                            content_hash: wi.content_hash,
                            name: func_ir.name.clone(),
                            params: func_ir.params.clone(),
                            ops: func_ir.ops.clone(),
                            param_types: func_ir.param_types.clone(),
                        }
                    })
                    .collect();

                // Each element: (func_index, content_hash, optimized_ops)
                // Use a custom thread pool with 16MB stacks for TIR.
                // lower_to_simple_ir has deeply nested closures capturing
                // many HashMaps, which exceeds rayon's default 8MB stacks.
                let tir_pool = rayon::ThreadPoolBuilder::new()
                    .stack_size(64 * 1024 * 1024)
                    .build()
                    .expect("Failed to build TIR thread pool");
                let results: Vec<(usize, String, Vec<OpIR>)> = tir_pool.install(|| {
                    inputs
                        .into_par_iter()
                        .map(|input| {
                            let idx = input.index;
                            let content_hash = input.content_hash;
                            // Build a temporary FunctionIR for the TIR pipeline.
                            let mut tmp_func = FunctionIR {
                                name: input.name,
                                params: input.params,
                                ops: input.ops,
                                param_types: input.param_types,
                                source_file: None,
                                is_extern: false,
                            };
                            if std::env::var("MOLT_TIR_TRACE_FUNC").as_deref() == Ok("1") {
                                eprintln!("[TIR-TRACE] {}", tmp_func.name);
                            }
                            // The TIR roundtrip linearizes structured control flow
                            // into jump/label blocks. Rewrite phi merges only for
                            // functions that actually enter that roundtrip so the
                            // state-machine backend does not see residual phi ops.
                            if tmp_func.ops.iter().any(|op| op.kind == "phi") {
                                rewrite_phi_to_store_load(&mut tmp_func.ops);
                            }
                            let func_name = tmp_func.name.clone();
                            let mut tir_func =
                                crate::tir::lower_from_simple::lower_to_tir(&tmp_func);
                            crate::tir::type_refine::refine_types(&mut tir_func);
                            let _stats = crate::tir::passes::run_pipeline(&mut tir_func);
                            crate::tir::type_refine::refine_types(&mut tir_func);
                            let type_map = if std::env::var("MOLT_TIR_NO_TYPES").is_ok() {
                                std::collections::HashMap::new()
                            } else {
                                crate::tir::type_refine::extract_type_map(&tir_func)
                            };
                            let lir_func =
                                crate::tir::lower_to_lir::lower_function_to_lir(&tir_func);
                            if let Err(errors) =
                                crate::tir::verify_lir::verify_lir_function(&lir_func)
                            {
                                panic!(
                                    "[LIR] verification failed for '{}': {:?}",
                                    func_name, errors
                                );
                            }
                            let ops = crate::tir::lower_to_simple::lower_to_simple_ir(
                                &tir_func, &type_map,
                            );
                            assert!(
                                crate::tir::lower_to_simple::validate_labels(&ops),
                                "TIR roundtrip emitted invalid labels for '{}'",
                                func_name
                            );
                            (idx, content_hash, ops)
                        })
                        .collect()
                });

                // Phase 3 (sequential): apply validated TIR ops and cache them.
                for (idx, content_hash, ops) in &results {
                    let _ = std::mem::replace(&mut ir.functions[*idx].ops, ops.clone());
                    tir_optimized_names.insert(ir.functions[*idx].name.clone());
                    let bytes = crate::tir::serialize::serialize_ops(ops);
                    tir_cache.put(content_hash, &bytes, vec![]);
                }

                let tir_elapsed = tir_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: TIR parallel optimization took {tir_elapsed:.2?} for {uncached_count} functions"
                );
            }

            tir_cache.save_index();
        }
        // Post-TIR: analysis + inlining (from main)
        // Capture task_kinds and task_closure_sizes BEFORE megafunction splitting.
        // Megafunction splitting can separate `func_new` from its corresponding
        // `set_attr_generic_obj(__molt_is_generator__)` into different chunk
        // functions, which breaks the per-function cross-reference in
        // `analyze_native_backend_ir`.  By capturing generator/coroutine
        // annotations now, we ensure they survive the split.
        let pre_split_task_kinds: BTreeMap<String, TrampolineKind>;
        let pre_split_task_closure_sizes: BTreeMap<String, i64>;
        {
            let analysis = analyze_native_backend_ir(&ir);
            pre_split_task_kinds = analysis.task_kinds;
            pre_split_task_closure_sizes = analysis.task_closure_sizes;
            if analysis.needs_inlining && !self.skip_ir_passes {
                inline_functions(&mut ir);
            }
        }
        // Dead function elimination: remove functions that are unreachable from
        // the entry point after inlining.  This reduces code size for both the
        // native object and the downstream linker's work.
        if !self.skip_ir_passes {
            eliminate_dead_functions(&mut ir);
        }
        // Megafunction splitting: break up functions with >4000 ops (or
        // MOLT_MAX_FUNCTION_OPS) into private chunk functions to avoid
        // Cranelift's O(n²) register allocator blowup.
        split_megafunctions(&mut ir);
        rewrite_annotate_stubs(&mut ir);
        for func in &mut ir.functions {
            rewrite_copy_aliases(&mut func.ops);
            if std::env::var("MOLT_DUMP_REWRITTEN_FUNC").as_deref() == Ok(func.name.as_str()) {
                let mut dump = String::new();
                for (idx, op) in func.ops.iter().enumerate() {
                    let _ = writeln!(dump, "{idx:04}: {:?}", op);
                }
                let _ = std::fs::write("tmp/rewritten_func_ir.txt", dump);
            }
        }
        if !self.skip_shared_stdlib_partition {
            externalize_shared_stdlib_partition(&mut ir);
        }
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }
        // ── LLVM backend dispatch ──
        // When MOLT_BACKEND=llvm and the llvm feature is compiled in, route
        // through the LLVM backend instead of Cranelift.  Each function is
        // lifted to TIR, lowered to LLVM IR, then the whole module is
        // optimized and emitted as a native object file.
        #[cfg(feature = "llvm")]
        if _use_llvm {
            use crate::llvm_backend::{LlvmBackend, MoltOptLevel};
            use crate::tir::lower_from_simple::lower_to_tir;

            let context = inkwell::context::Context::create();
            let mut llvm = LlvmBackend::new(&context, "molt_module");

            // Declare all runtime functions that lowered code may call into.
            crate::llvm_backend::runtime_imports::declare_runtime_functions(
                llvm.context,
                &llvm.module,
            );

            let func_count = ir.functions.iter().filter(|f| !f.is_extern).count();
            let total_ops: usize = ir
                .functions
                .iter()
                .filter(|f| !f.is_extern)
                .map(|f| f.ops.len())
                .sum();
            eprintln!(
                "MOLT_BACKEND(llvm): compiling {func_count} functions ({total_ops} total ops)"
            );
            let codegen_start = std::time::Instant::now();

            let tir_funcs: Vec<_> = ir
                .functions
                .iter()
                .map(|func| (func.is_extern, lower_to_tir(func)))
                .collect();
            llvm.function_param_types = tir_funcs
                .iter()
                .map(|(_, func)| (func.name.clone(), func.param_types.clone()))
                .collect();
            llvm.function_return_types = tir_funcs
                .iter()
                .map(|(_, func)| (func.name.clone(), func.return_type.clone()))
                .collect();

            for (_, tir_func) in &tir_funcs {
                crate::llvm_backend::lowering::declare_tir_function(tir_func, &llvm);
            }

            for (is_extern, tir_func) in &tir_funcs {
                if *is_extern {
                    continue;
                }
                if env_setting("TIR_DUMP").as_deref() == Some("1")
                    || env_setting("MOLT_TIR_DUMP").as_deref() == Some("1")
                {
                    eprintln!(
                        "[LLVM] TIR for '{}':\n{}",
                        tir_func.name,
                        crate::tir::printer::print_function(tir_func)
                    );
                }
                crate::llvm_backend::lowering::lower_tir_to_llvm(tir_func, &llvm);
            }

            // Dump LLVM IR under the repo-local debug artifact root when
            // MOLT_LLVM_DUMP_IR=1.
            let dump_ir = env_setting("MOLT_LLVM_DUMP_IR").as_deref() == Some("1");
            if dump_ir {
                let _ = crate::debug_artifacts::write_debug_artifact(
                    "llvm/before_opt.ll",
                    llvm.dump_ir(),
                );
            }

            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed before optimization:\n{}",
                    msg.to_string()
                )
            });

            llvm.optimize(MoltOptLevel::Aggressive);
            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed after optimization:\n{}",
                    msg.to_string()
                )
            });

            if dump_ir {
                let _ = crate::debug_artifacts::write_debug_artifact(
                    "llvm/after_opt.ll",
                    llvm.dump_ir(),
                );
            }

            if timing {
                let codegen_elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: LLVM codegen + optimization took {codegen_elapsed:.2?}"
                );
            }

            let tmp_obj = crate::debug_artifacts::prepare_unique_debug_artifact_path(
                "llvm/molt_llvm_output.o",
            )
            .expect("failed to prepare LLVM object path");
            llvm.emit_object(&tmp_obj, MoltOptLevel::Aggressive)
                .expect("LLVM object emission failed");
            let bytes = std::fs::read(&tmp_obj).unwrap_or_else(|err| {
                panic!(
                    "failed to read LLVM object file at {}: {}",
                    tmp_obj.display(),
                    err
                )
            });
            let _ = std::fs::remove_file(&tmp_obj);

            if timing {
                let total_elapsed = compile_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: total LLVM backend compile: {total_elapsed:.2?}                      ({func_count} functions, {total_ops} ops, {} bytes)",
                    bytes.len()
                );
            }

            return bytes;
        }
        // Re-analyze after dead function elimination and megafunction
        // splitting so defined_functions/closure_functions reflect only the
        // surviving (and newly created chunk) functions.
        let mut ir_analysis = analyze_native_backend_ir(&ir);
        // Merge pre-split task annotations: megafunction splitting can
        // separate `func_new` from `set_attr_generic_obj(__molt_is_generator__)`
        // into different chunk functions, causing the post-split analysis to
        // miss generator/coroutine annotations.  The pre-split analysis
        // captured these correctly before the ops were split apart.
        for (name, kind) in &pre_split_task_kinds {
            ir_analysis.task_kinds.entry(name.clone()).or_insert(*kind);
        }
        for (name, size) in &pre_split_task_closure_sizes {
            ir_analysis
                .task_closure_sizes
                .entry(name.clone())
                .or_insert(*size);
        }
        // Conditional trace elimination: skip emitting trace_enter/trace_exit calls
        // when tracing is disabled. Each guarded call site emits 2 trace function calls
        // (enter + exit); eliminating them saves codegen work on cache misses and
        // keeps the default native backend lane focused on production semantics.
        // Trace emission is opt-in via MOLT_BACKEND_EMIT_TRACES=1.
        let emit_traces = env_setting("MOLT_BACKEND_EMIT_TRACES")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        // Compile functions. For large modules (>128 functions), use the
        // Cranelift catch_unwind resilience path that retries failing
        // functions at opt_level=none.  The single-module approach is
        // retained (no batching) because Cranelift 0.130's ObjectModule
        // handles large function counts efficiently when individual
        // function compilations are bounded.
        // Register extern functions (bodies in stdlib_shared.o) so the
        // backend declares them as Import linkage, resolved by the linker.
        for func in &ir.functions {
            if func.is_extern {
                self.external_function_names.insert(func.name.clone());
            }
        }
        // Filter out extern functions — they have no ops to compile.
        ir.functions.retain(|f| !f.is_extern);
        let func_count = ir.functions.len();
        let total_ops: usize = ir.functions.iter().map(|f| f.ops.len()).sum();
        eprintln!("MOLT_BACKEND: compiling {func_count} functions ({total_ops} total ops)");
        let codegen_start = std::time::Instant::now();
        let local_function_arities: BTreeMap<String, usize> = ir
            .functions
            .iter()
            .map(|func| (func.name.clone(), func.params.len()))
            .collect();
        let local_return_alias_summaries =
            crate::passes::compute_return_alias_summaries(&ir.functions);
        let module_context = self.module_context.clone();
        let effective_function_arities =
            merge_function_arities(module_context.as_ref(), local_function_arities);
        let effective_closure_functions = module_context
            .as_ref()
            .map(|context| context.closure_functions.clone())
            .unwrap_or_else(|| ir_analysis.closure_functions.clone());
        let effective_task_kinds = module_context
            .as_ref()
            .map(|context| context.task_kinds.clone())
            .unwrap_or_else(|| ir_analysis.task_kinds.clone());
        let effective_task_closure_sizes = module_context
            .as_ref()
            .map(|context| context.task_closure_sizes.clone())
            .unwrap_or_else(|| ir_analysis.task_closure_sizes.clone());
        let effective_leaf_functions = module_context
            .as_ref()
            .map(|context| context.leaf_functions.clone())
            .unwrap_or_else(|| ir_analysis.leaf_functions.clone());
        let effective_return_alias_summaries = module_context
            .as_ref()
            .map(|context| context.return_alias_summaries.clone())
            .unwrap_or(local_return_alias_summaries);
        let local_function_has_ret = compute_function_has_ret(&ir.functions);
        let effective_function_has_ret =
            merge_function_has_ret(module_context.as_ref(), local_function_has_ret);
        let mut module_known_functions = ir_analysis.defined_functions.clone();
        module_known_functions.extend(self.external_function_names.iter().cloned());
        let mut compiled = 0u32;
        let failed = 0u32;
        let mut slowest_func: Option<(String, std::time::Duration)> = None;
        // Progress reporting: pick interval based on function count so the
        // user sees roughly 20 updates during a long build, but at least
        // every 50 functions.
        let progress_interval = (func_count / 20).clamp(1, 50);
        let mut last_progress = std::time::Instant::now();

        for func_ir in ir.functions {
            let func_name = func_ir.name.clone();
            let func_start = std::time::Instant::now();
            self.compile_func(
                func_ir,
                &effective_task_kinds,
                &effective_task_closure_sizes,
                &ir_analysis.defined_functions,
                &module_known_functions,
                &effective_closure_functions,
                &effective_return_alias_summaries,
                emit_traces,
                &effective_leaf_functions,
                &effective_function_arities,
                &effective_function_has_ret,
            );
            let func_elapsed = func_start.elapsed();
            if timing && func_elapsed.as_millis() > 500 {
                eprintln!("MOLT_BACKEND_TIMING: function `{func_name}` took {func_elapsed:.2?}");
            }
            if slowest_func.as_ref().is_none_or(|(_, d)| func_elapsed > *d) {
                slowest_func = Some((func_name, func_elapsed));
            }
            compiled += 1;
            // Print progress at regular intervals, or every 500ms for
            // slow builds where individual functions take a long time.
            if (compiled as usize).is_multiple_of(progress_interval)
                || last_progress.elapsed().as_millis() >= 500
            {
                let pct = (compiled as f64 / func_count as f64 * 100.0) as u32;
                let elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: [{pct:3}%] compiled {compiled}/{func_count} functions ({elapsed:.1?} elapsed)"
                );
                last_progress = std::time::Instant::now();
            }
        }
        if timing {
            let codegen_elapsed = codegen_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: Cranelift codegen took {codegen_elapsed:.2?}");
            if let Some((name, dur)) = &slowest_func {
                eprintln!("MOLT_BACKEND_TIMING: slowest function: `{name}` ({dur:.2?})");
            }
        }
        if failed > 0 {
            eprintln!("MOLT_BACKEND: {failed} functions failed, {compiled} succeeded");
        }
        // ── Parallel Cranelift compilation ────────────────────────
        // All functions were IR-built sequentially above (declarations
        // and Cranelift IR construction are not thread-safe), but actual
        // machine-code compilation (register allocation, instruction
        // selection, encoding) is deferred.  Flush them now in parallel.
        {
            let deferred_count = self.deferred_defines.len();
            if deferred_count > 0 {
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: parallel Cranelift flush ({deferred_count} functions) took {flush_elapsed:.2?}"
                    );
                }
            }
        }
        // ── Post-compilation: define trap stubs for declared-but-undefined
        // functions.  This covers `__ov{N}` variants created when a function
        // is referenced with different arities, and functions that were skipped
        // due to signature mismatches or compilation failures.
        let mut stubs_emitted = 0u32;
        let declared: Vec<(
            cranelift_module::FuncId,
            String,
            cranelift_codegen::ir::Signature,
        )> = self
            .module
            .declarations()
            .get_functions()
            .filter_map(|(fid, decl)| {
                let name = decl.name.clone()?;
                if decl.linkage == cranelift_module::Linkage::Export
                    && !self.defined_func_names.contains(&name)
                {
                    Some((fid, name, decl.signature.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (fid, name, sig) in declared {
            // In batched compilation, skip trap stubs for functions that
            // exist in other batches — ld -r will resolve them at merge
            // time.  But functions that don't exist in ANY batch (like
            // __ov variants or internally-generated names) still need
            // stubs to avoid Cranelift "Export must be defined" panics.
            if !self.external_function_names.is_empty()
                && self.external_function_names.contains(&name)
            {
                // Function exists in another batch — ld -r will provide
                // the real definition.  Downgrade from Export to Import
                // so Cranelift's ObjectModule doesn't require a body.
                let _ =
                    self.module
                        .declare_function(&name, cranelift_module::Linkage::Import, &sig);
                continue;
            }
            if let Err(e) = Self::emit_trap_stub(&mut self.module, fid, &sig, &name) {
                eprintln!("WARNING: failed to emit trap stub for `{}`: {}", name, e);
                // Trap stub failed (function may already be defined with a
                // different body, or another edge case).  As a last resort,
                // try to downgrade the linkage to Import so `finish()` does
                // not panic with "Export must be defined."
                let _ =
                    self.module
                        .declare_function(&name, cranelift_module::Linkage::Import, &sig);
            } else {
                stubs_emitted += 1;
            }
        }
        if stubs_emitted > 0 {
            eprintln!(
                "WARNING: emitted {} trap stub(s) for declared-but-undefined functions",
                stubs_emitted
            );
        }

        let emit_start = std::time::Instant::now();
        let mut product = self.module.finish();
        // Set MachO platform load command so ld doesn't emit
        // "no platform load command found" warnings on macOS.
        #[cfg(target_os = "macos")]
        {
            use cranelift_object::object::write::MachOBuildVersion;
            // Encode macOS 11.0.0 as minimum deployment target.
            // Version encoding: xxxx.yy.zz nibbles => 0x000B0000 = 11.0.0
            let mut bv = MachOBuildVersion::default();
            bv.platform = cranelift_object::object::macho::PLATFORM_MACOS;
            bv.minos = 0x000B_0000; // macOS 11.0.0
            bv.sdk = 0; // no SDK constraint
            product.object.set_macho_build_version(bv);
        }
        let bytes = product.emit().unwrap();
        if timing {
            let emit_elapsed = emit_start.elapsed();
            let total_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: object emit took {emit_elapsed:.2?}");
            eprintln!(
                "MOLT_BACKEND_TIMING: total backend compile: {total_elapsed:.2?} \
                 ({func_count} functions, {total_ops} ops, {} bytes)",
                bytes.len()
            );
        }
        bytes
    }

    fn ensure_trampoline(
        module: &mut ObjectModule,
        trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
        func_name: &str,
        linkage: Linkage,
        spec: TrampolineSpec,
    ) -> cranelift_module::FuncId {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret,
        } = spec;
        let is_import = matches!(linkage, Linkage::Import);
        let key = TrampolineKey {
            name: func_name.to_string(),
            arity,
            has_closure,
            is_import,
            kind,
            closure_size,
            target_has_ret,
        };
        if let Some(id) = trampoline_ids.get(&key) {
            return *id;
        }
        let closure_suffix = if has_closure { "_closure" } else { "" };
        let import_suffix = if is_import { "_import" } else { "" };
        let ret_suffix = if target_has_ret { "" } else { "_void" };
        let kind_suffix = match kind {
            TrampolineKind::Plain => "",
            TrampolineKind::Generator => "_gen",
            TrampolineKind::Coroutine => "_coro",
            TrampolineKind::AsyncGen => "_asyncgen",
        };
        let trampoline_name = format!(
            "{func_name}__molt_trampoline_{arity}{closure_suffix}{kind_suffix}{ret_suffix}{import_suffix}"
        );
        let mut ctx = module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        let nbc = NanBoxConsts::new(&mut builder);

        let closure_bits = builder.block_params(entry_block)[0];
        let args_ptr = builder.block_params(entry_block)[1];
        let _args_len = builder.block_params(entry_block)[2];

        let poll_target = if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            if func_name.ends_with("_poll") {
                func_name.to_string()
            } else {
                format!("{func_name}_poll")
            }
        } else {
            String::new()
        };

        match kind {
            TrampolineKind::Generator => {
                if closure_size < 0 {
                    panic!("generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlags::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), args_ptr, arg_offset);
                    builder
                        .ins()
                        .store(MemFlags::trusted(), arg_val, obj_ptr, offset + arg_offset);
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }
                builder.ins().return_(&[obj]);
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

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_COROUTINE);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                if payload_slots > 0 {
                    let mut inc_ref_obj_sig = module.make_signature();
                    inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                    let inc_ref_obj_callee = module
                        .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                        .unwrap();
                    let local_inc_ref_obj =
                        module.declare_func_in_func(inc_ref_obj_callee, builder.func);
                    let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                    let mut offset = 0i32;
                    if has_closure {
                        builder
                            .ins()
                            .store(MemFlags::trusted(), closure_bits, obj_ptr, offset);
                        builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                        offset += 8;
                    }
                    for idx in 0..arity {
                        let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                        let arg_val = builder.ins().load(
                            types::I64,
                            MemFlags::trusted(),
                            args_ptr,
                            arg_offset,
                        );
                        builder.ins().store(
                            MemFlags::trusted(),
                            arg_val,
                            obj_ptr,
                            offset + arg_offset,
                        );
                        builder.ins().call(local_inc_ref_obj, &[arg_val]);
                    }
                }

                let mut get_sig = module.make_signature();
                get_sig.returns.push(AbiParam::new(types::I64));
                let get_callee = module
                    .declare_function("molt_cancel_token_get_current", Linkage::Import, &get_sig)
                    .unwrap();
                let get_local = module.declare_func_in_func(get_callee, builder.func);
                let get_call = builder.ins().call(get_local, &[]);
                let current_token = builder.inst_results(get_call)[0];

                let mut reg_sig = module.make_signature();
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.returns.push(AbiParam::new(types::I64));
                let reg_callee = module
                    .declare_function("molt_task_register_token_owned", Linkage::Import, &reg_sig)
                    .unwrap();
                let reg_local = module.declare_func_in_func(reg_callee, builder.func);
                builder.ins().call(reg_local, &[obj, current_token]);

                builder.ins().return_(&[obj]);
            }
            TrampolineKind::AsyncGen => {
                if closure_size < 0 {
                    panic!("async generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("async generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlags::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), args_ptr, arg_offset);
                    builder
                        .ins()
                        .store(MemFlags::trusted(), arg_val, obj_ptr, offset + arg_offset);
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }

                let mut asyncgen_sig = module.make_signature();
                asyncgen_sig.params.push(AbiParam::new(types::I64));
                asyncgen_sig.returns.push(AbiParam::new(types::I64));
                let asyncgen_callee = module
                    .declare_function("molt_asyncgen_new", Linkage::Import, &asyncgen_sig)
                    .unwrap();
                let asyncgen_local = module.declare_func_in_func(asyncgen_callee, builder.func);
                let asyncgen_call = builder.ins().call(asyncgen_local, &[obj]);
                let asyncgen_obj = builder.inst_results(asyncgen_call)[0];
                builder.ins().return_(&[asyncgen_obj]);
            }
            TrampolineKind::Plain => {
                let mut call_args = Vec::with_capacity(arity + if has_closure { 1 } else { 0 });
                if has_closure {
                    call_args.push(closure_bits);
                }
                for idx in 0..arity {
                    let offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), args_ptr, offset);
                    call_args.push(arg_val);
                }

                let mut target_sig = module.make_signature();
                if has_closure {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                for _ in 0..arity {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                if target_has_ret {
                    target_sig.returns.push(AbiParam::new(types::I64));
                }
                // Always use Import for the target function inside
                // trampolines: the target is defined by its own
                // compile_func call (Export), and in batched compilation
                // the target may be in a different batch .o file.
                let target_id = module
                    .declare_function(func_name, Linkage::Import, &target_sig)
                    .unwrap();
                let target_ref = module.declare_func_in_func(target_id, builder.func);
                let call = builder.ins().call(target_ref, &call_args);
                if target_has_ret {
                    let res = builder.inst_results(call)[0];
                    builder.ins().return_(&[res]);
                } else {
                    let none_val = builder.ins().iconst(types::I64, box_none());
                    builder.ins().return_(&[none_val]);
                }
            }
        }

        builder.seal_all_blocks();
        builder.finalize();

        let trampoline_id = module
            .declare_function(&trampoline_name, Linkage::Local, &ctx.func.signature)
            .unwrap();
        if let Err(err) = module.define_function(trampoline_id, &mut ctx) {
            panic!("Failed to define trampoline {trampoline_name}: {err:?}");
        }
        trampoline_ids.insert(key, trampoline_id);
        trampoline_id
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::{
        FunctionIR, NativeBackendModuleContext, OpIR, SimpleBackend, SimpleIR, TrampolineKind,
        analyze_native_backend_ir, compute_function_has_ret, merge_function_arities,
        merge_function_has_ret,
    };
    use crate::drain_cleanup_entry_tracked;
    use crate::passes::ReturnAliasSummary;
    use crate::rewrite_phi_to_store_load;
    use cranelift_codegen::ir::Value;
    use cranelift_codegen::ir::types;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, OnceLock};

    fn backend_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn compile_trace_probe_object(emit_traces_env: Option<&str>) -> Vec<u8> {
        let _guard = backend_env_lock().lock().expect("env lock poisoned");
        // Disable TIR for these tests — they test native backend import emission,
        // not the optimisation pipeline.
        unsafe { std::env::set_var("MOLT_TIR_OPT", "0") };
        match emit_traces_env {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_EMIT_TRACES", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") },
        }
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };
        let bytes = SimpleBackend::new().compile(ir);
        unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") };
        unsafe { std::env::remove_var("MOLT_TIR_OPT") };
        bytes
    }

    fn compile_function_to_clif_text(functions: Vec<FunctionIR>, target_name: &str) -> String {
        let ir = SimpleIR {
            functions,
            profile: None,
        };
        let analysis = analyze_native_backend_ir(&ir);
        let function_has_ret = compute_function_has_ret(&ir.functions);
        let function_arities = ir
            .functions
            .iter()
            .map(|func| (func.name.clone(), func.params.len()))
            .collect();
        let return_alias_summaries = crate::passes::compute_return_alias_summaries(&ir.functions);
        let target_func = ir
            .functions
            .into_iter()
            .find(|func| func.name == target_name)
            .unwrap_or_else(|| panic!("missing target function `{target_name}`"));
        let mut backend = SimpleBackend::new();
        backend.compile_func(
            target_func,
            &analysis.task_kinds,
            &analysis.task_closure_sizes,
            &analysis.defined_functions,
            &analysis.defined_functions,
            &analysis.closure_functions,
            &return_alias_summaries,
            false,
            &analysis.leaf_functions,
            &function_arities,
            &function_has_ret,
        );
        backend
            .deferred_defines
            .iter()
            .find(|deferred| deferred.name == target_name)
            .unwrap_or_else(|| panic!("missing deferred function `{target_name}`"))
            .func
            .display()
            .to_string()
    }

    fn roundtrip_function_through_tir(func: &FunctionIR) -> FunctionIR {
        let mut tir = crate::tir::lower_from_simple::lower_to_tir(func);
        crate::tir::type_refine::refine_types(&mut tir);
        let _stats = crate::tir::passes::run_pipeline(&mut tir);
        crate::tir::type_refine::refine_types(&mut tir);
        let type_map = crate::tir::type_refine::extract_type_map(&tir);
        let lir = crate::tir::lower_to_lir::lower_function_to_lir(&tir);
        assert!(
            crate::tir::verify_lir::verify_lir_function(&lir).is_ok(),
            "LIR verification failed after TIR optimization"
        );
        let ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir, &type_map);
        assert!(
            crate::tir::lower_to_simple::validate_labels(&ops),
            "TIR roundtrip must preserve all referenced labels: {ops:#?}"
        );
        FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops,
            param_types: func.param_types.clone(),
            source_file: func.source_file.clone(),
            is_extern: false,
        }
    }

    #[test]
    fn native_backend_skips_trace_imports_by_default() {
        let bytes = compile_trace_probe_object(None);

        assert!(
            !bytes
                .windows(b"molt_trace_enter_slot".len())
                .any(|window| window == b"molt_trace_enter_slot")
        );
        assert!(
            !bytes
                .windows(b"molt_trace_exit".len())
                .any(|window| window == b"molt_trace_exit")
        );
    }

    #[test]
    fn native_backend_can_opt_in_trace_imports() {
        let bytes = compile_trace_probe_object(Some("1"));

        assert!(
            bytes
                .windows(b"molt_trace_enter_slot".len())
                .any(|window| window == b"molt_trace_enter_slot")
        );
        assert!(
            bytes
                .windows(b"molt_trace_exit".len())
                .any(|window| window == b"molt_trace_exit")
        );
    }

    #[test]
    fn native_backend_ir_analysis_skips_inlining_without_internal_calls() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let analysis = analyze_native_backend_ir(&ir);

        assert!(!analysis.needs_inlining);
        assert!(analysis.defined_functions.contains("molt_main"));
    }

    #[test]
    fn native_backend_ir_analysis_collects_task_metadata_once_needed() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        out: Some("flag".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("closure_size".to_string()),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new_closure".to_string(),
                        out: Some("poll_obj".to_string()),
                        s_value: Some("worker_poll".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        s_value: Some("__molt_is_coroutine__".to_string()),
                        args: Some(vec!["poll_obj".to_string(), "flag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        s_value: Some("__molt_closure_size__".to_string()),
                        args: Some(vec!["poll_obj".to_string(), "closure_size".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let analysis = analyze_native_backend_ir(&ir);

        assert!(analysis.closure_functions.contains("worker_poll"));
        assert_eq!(
            analysis.task_kinds.get("worker_poll"),
            Some(&TrampolineKind::Coroutine)
        );
        assert_eq!(analysis.task_closure_sizes.get("worker_poll"), Some(&3));
    }

    #[test]
    fn native_backend_module_context_preserves_cross_batch_alias_metadata() {
        let functions = vec![
            FunctionIR {
                name: "helper".to_string(),
                params: vec!["value".to_string(), "intrinsic".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    var: Some("value".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "helper_poll".to_string(),
                params: vec!["state".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    var: Some("state".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let context = SimpleBackend::build_module_context(&functions);

        assert_eq!(context.function_arities.get("helper"), Some(&2));
        assert_eq!(context.function_has_ret.get("helper"), Some(&true));
        assert_eq!(
            context.return_alias_summaries.get("helper"),
            Some(&ReturnAliasSummary::Param(0))
        );
        assert!(context.leaf_functions.contains("helper"));
        assert!(context.leaf_functions.contains("helper_poll"));
    }

    #[test]
    fn tir_roundtrip_preserves_store_var_return_alias_summary() {
        let func = FunctionIR {
            name: "helper".to_string(),
            params: vec!["value".to_string()],
            ops: vec![
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("tmp".to_string()),
                    args: Some(vec!["value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("tmp".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["str".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let roundtripped = roundtrip_function_through_tir(&func);
        let summaries =
            crate::passes::compute_return_alias_summaries(std::slice::from_ref(&roundtripped));

        assert_eq!(
            summaries.get("helper"),
            Some(&ReturnAliasSummary::Param(0)),
            "roundtripped params: {:?}; ops: {:?}; summaries: {:?}",
            roundtripped.params,
            roundtripped.ops,
            summaries
        );
    }

    #[test]
    fn native_backend_module_context_preserves_cross_batch_void_return_metadata() {
        let functions = vec![
            FunctionIR {
                name: "value_helper".to_string(),
                params: vec!["value".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    var: Some("value".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "void_helper".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let context = SimpleBackend::build_module_context(&functions);

        assert_eq!(context.function_has_ret.get("value_helper"), Some(&true));
        assert_eq!(context.function_has_ret.get("void_helper"), Some(&false));
    }

    #[test]
    fn trampoline_key_distinguishes_void_and_value_targets() {
        let value_key = crate::TrampolineKey {
            name: "helper".to_string(),
            arity: 1,
            has_closure: false,
            is_import: false,
            kind: TrampolineKind::Plain,
            closure_size: 0,
            target_has_ret: true,
        };
        let void_key = crate::TrampolineKey {
            target_has_ret: false,
            ..value_key.clone()
        };

        assert_ne!(value_key, void_key);
    }

    #[test]
    fn native_backend_preserves_split_stub_calls_to_void_and_value_chunks() {
        let chunk0 = "__molt_chunk_demo__molt_module_chunk_1_0".to_string();
        let chunk1 = "__molt_chunk_demo__molt_module_chunk_1_1".to_string();
        let stub = "demo__molt_module_chunk_1".to_string();
        let clif = compile_function_to_clif_text(
            vec![
                FunctionIR {
                    name: chunk0,
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: chunk1,
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "const".to_string(),
                            out: Some("chunk_ret".to_string()),
                            value: Some(7),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("chunk_ret".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: stub.clone(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("__molt_chunk_demo__molt_module_chunk_1_0".to_string()),
                            out: Some("__chunk_discard_0".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("__molt_chunk_demo__molt_module_chunk_1_1".to_string()),
                            out: Some("__chunk_ret".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("__chunk_ret".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            &stub,
        );
        let local_callees: Vec<String> = clif
            .lines()
            .map(str::trim)
            .filter_map(|line| {
                line.split_once(" = colocated")
                    .map(|(name, _)| name.to_string())
            })
            .collect();
        assert_eq!(
            local_callees.len(),
            2,
            "stub CLIF should reference exactly two local chunk callees:\n{clif}",
        );
        assert!(
            local_callees
                .iter()
                .any(|callee| clif.contains(&format!("call {callee}("))),
            "split stub must retain the direct call to the first void-returning chunk:\n{clif}",
        );
        assert!(
            local_callees
                .iter()
                .any(|callee| clif.contains(&format!("= call {callee}("))),
            "split stub must retain the direct call to the final value-returning chunk:\n{clif}",
        );
    }

    #[test]
    fn compute_function_has_ret_uses_actual_ir_not_name_heuristics() {
        let result = compute_function_has_ret(&[
            FunctionIR {
                name: "demo__molt_module_chunk_1".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "demo____molt_globals_builtin__".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("ret".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("ret".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ]);

        assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
        assert_eq!(result.get("demo____molt_globals_builtin__"), Some(&true));
    }

    #[test]
    fn compute_function_has_ret_keeps_actual_signature_for_python_callable_targets() {
        let result = compute_function_has_ret(&[
            FunctionIR {
                name: "user_func".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "demo__molt_module_chunk_1".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "func_new".to_string(),
                    s_value: Some("user_func".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ]);

        assert_eq!(result.get("user_func"), Some(&false));
        assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
    }

    #[test]
    fn local_function_metadata_overrides_stale_module_context_after_split() {
        let context = NativeBackendModuleContext {
            function_arities: BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                1usize,
            )]),
            function_has_ret: BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                false,
            )]),
            ..NativeBackendModuleContext::default()
        };

        let merged_arities = merge_function_arities(
            Some(&context),
            BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                1usize,
            )]),
        );
        let merged_has_ret = merge_function_has_ret(
            Some(&context),
            BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                true,
            )]),
        );

        assert_eq!(
            merged_arities.get("__molt_chunk_builtins__molt_module_chunk_3_0"),
            Some(&1usize)
        );
        assert_eq!(
            merged_has_ret.get("__molt_chunk_builtins__molt_module_chunk_3_0"),
            Some(&true)
        );
    }

    #[test]
    fn native_backend_import_ids_are_cached_by_symbol() {
        let mut backend = SimpleBackend::new();

        let first = backend.import_func_id("molt_dec_ref", &[types::I64], &[]);
        let second = backend.import_func_id("molt_dec_ref", &[types::I64], &[]);

        assert_eq!(first, second);
        assert_eq!(backend.import_ids.len(), 1);
    }

    #[test]
    fn native_backend_skips_profile_store_imports_when_function_has_no_store_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(
            !bytes
                .windows(b"molt_profile_struct_field_store".len())
                .any(|window| window == b"molt_profile_struct_field_store")
        );
        assert!(
            !bytes
                .windows(b"molt_profile_enabled".len())
                .any(|window| window == b"molt_profile_enabled")
        );
    }

    #[test]
    fn native_backend_keeps_profile_store_imports_when_function_has_store_ops() {
        let _guard = backend_env_lock().lock().expect("env lock poisoned");
        // Disable TIR for this test — it tests native backend import emission.
        unsafe { std::env::set_var("MOLT_TIR_OPT", "0") };
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("obj".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("value".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store".to_string(),
                        args: Some(vec!["obj".to_string(), "value".to_string()]),
                        value: Some(8),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);
        unsafe { std::env::remove_var("MOLT_TIR_OPT") };

        assert!(
            bytes
                .windows(b"molt_profile_struct_field_store".len())
                .any(|window| window == b"molt_profile_struct_field_store")
        );
        assert!(
            bytes
                .windows(b"molt_profile_enabled".len())
                .any(|window| window == b"molt_profile_enabled")
        );
    }

    #[test]
    fn native_backend_compiles_exception_label_guard_if_without_else() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hello_regress____molt_globals_builtin__".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "exception_stack_enter".to_string(),
                        out: Some("v74".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_depth".to_string(),
                        out: Some("v75".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v76".to_string()),
                        s_value: Some("hello_regress".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_get".to_string(),
                        out: Some("v77".to_string()),
                        args: Some(vec!["v76".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v78".to_string()),
                        s_value: Some("__dict__".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        out: Some("v79".to_string()),
                        args: Some(vec!["v77".to_string(), "v78".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("v79".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_set_depth".to_string(),
                        args: Some(vec!["v75".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_exit".to_string(),
                        args: Some(vec!["v74".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".to_string(),
                        out: Some("v80".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v81".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".to_string(),
                        out: Some("v82".to_string()),
                        args: Some(vec!["v80".to_string(), "v81".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "not".to_string(),
                        out: Some("v83".to_string()),
                        args: Some(vec!["v82".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["v83".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".to_string(),
                        args: Some(vec!["v80".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v84".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("v84".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(!bytes.is_empty());
    }

    #[test]
    fn native_backend_compiles_tir_roundtripped_exception_label_guard_if_without_else() {
        let func = FunctionIR {
            name: "hello_regress____molt_globals_builtin__".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "exception_stack_enter".to_string(),
                    out: Some("v74".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_depth".to_string(),
                    out: Some("v75".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v76".to_string()),
                    s_value: Some("hello_regress".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    out: Some("v77".to_string()),
                    args: Some(vec!["v76".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v78".to_string()),
                    s_value: Some("__dict__".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    out: Some("v79".to_string()),
                    args: Some(vec!["v77".to_string(), "v78".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("v79".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".to_string(),
                    args: Some(vec!["v75".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_exit".to_string(),
                    args: Some(vec!["v74".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("v80".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v81".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    out: Some("v82".to_string()),
                    args: Some(vec!["v80".to_string(), "v81".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "not".to_string(),
                    out: Some("v83".to_string()),
                    args: Some(vec!["v82".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["v83".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".to_string(),
                    args: Some(vec!["v80".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let roundtripped = roundtrip_function_through_tir(&func);
        let clif = compile_function_to_clif_text(
            vec![roundtripped],
            "hello_regress____molt_globals_builtin__",
        );

        assert!(
            clif.contains("return"),
            "TIR-roundtripped exception function must compile to CLIF:\n{clif}"
        );
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn llvm_backend_keeps_shared_stdlib_partition_external() {
        let _guard = backend_env_lock().lock().expect("env lock poisoned");
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-llvm-stdlib-extern-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib_obj = tmp_dir.join("stdlib.o");
        std::fs::write(&stdlib_obj, b"placeholder").expect("write stdlib marker");

        let prev_backend = std::env::var("MOLT_BACKEND").ok();
        let prev_stdlib_obj = std::env::var("MOLT_STDLIB_OBJ").ok();
        let prev_entry_module = std::env::var("MOLT_ENTRY_MODULE").ok();
        let prev_stdlib_symbols = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok();
        unsafe {
            std::env::set_var("MOLT_BACKEND", "llvm");
            std::env::set_var("MOLT_STDLIB_OBJ", &stdlib_obj);
            std::env::set_var("MOLT_ENTRY_MODULE", "app");
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
        }

        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("molt_init_sys".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);
        let output = tmp_dir.join("out.o");
        std::fs::write(&output, &bytes).expect("write llvm object");
        let nm = std::process::Command::new("nm")
            .args(["-g", output.to_str().expect("utf8 object path")])
            .output()
            .expect("run nm");
        assert!(
            nm.status.success(),
            "nm failed: {}",
            String::from_utf8_lossy(&nm.stderr)
        );
        let symbols = String::from_utf8_lossy(&nm.stdout);
        assert!(
            symbols
                .lines()
                .any(|line| line.contains(" U _molt_init_sys")
                    || line == "                 U molt_init_sys"),
            "shared stdlib symbol must be an undefined external, got:\n{symbols}"
        );
        assert!(
            !symbols
                .lines()
                .any(|line| line.contains(" T _molt_init_sys") || line.contains(" T molt_init_sys")),
            "LLVM output object must not define shared stdlib symbol, got:\n{symbols}"
        );

        match prev_backend {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND") },
        }
        match prev_stdlib_obj {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_OBJ", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_OBJ") },
        }
        match prev_entry_module {
            Some(value) => unsafe { std::env::set_var("MOLT_ENTRY_MODULE", value) },
            None => unsafe { std::env::remove_var("MOLT_ENTRY_MODULE") },
        }
        match prev_stdlib_symbols {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS") },
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn native_backend_compiles_tir_roundtripped_nested_loops() {
        let func = FunctionIR {
            name: "nested_loops".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(2),
                    out: Some("outer_limit".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(2),
                    out: Some("inner_limit".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["i".into(), "outer_limit".into()]),
                    out: Some("outer_cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["outer_cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("j".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["j".into(), "inner_limit".into()]),
                    out: Some("inner_cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["inner_cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["total".into(), "j".into()]),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["j".into(), "one".into()]),
                    out: Some("j".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["i".into(), "one".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("total".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let roundtripped = roundtrip_function_through_tir(&func);
        let clif = compile_function_to_clif_text(vec![roundtripped], "nested_loops");

        assert!(
            clif.contains("return"),
            "TIR-roundtripped nested-loop function must compile to CLIF:\n{clif}"
        );
    }

    #[test]
    fn annotate_function_object_compiles_without_signature_mismatch() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "_sitebuiltins____annotate__".to_string(),
                    params: vec!["format".to_string()],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "func_new".to_string(),
                            s_value: Some("_sitebuiltins____annotate__".to_string()),
                            value: Some(1),
                            out: Some("annotate_fn".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(!bytes.is_empty());
    }

    #[test]
    fn guarded_void_function_object_compiles_without_result_panic() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "void_helper".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "func_new".to_string(),
                            s_value: Some("void_helper".to_string()),
                            value: Some(0),
                            out: Some("void_fn".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "call_guarded".to_string(),
                            s_value: Some("void_helper".to_string()),
                            args: Some(vec!["void_fn".to_string()]),
                            out: Some("result".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("result".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(!bytes.is_empty());
    }

    #[test]
    fn direct_imported_runtime_call_avoids_guarded_call_wrapper() {
        let func = FunctionIR {
            name: "hot_runtime_call".to_string(),
            params: vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
                "f".to_string(),
                "g".to_string(),
                "h".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("molt_gpu_linear_contiguous".to_string()),
                    args: Some(vec![
                        "a".to_string(),
                        "b".to_string(),
                        "c".to_string(),
                        "d".to_string(),
                        "e".to_string(),
                        "f".to_string(),
                        "g".to_string(),
                        "h".to_string(),
                    ]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("out".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let clif = compile_function_to_clif_text(vec![func], "hot_runtime_call");

        assert!(
            !clif.contains("molt_guarded_call"),
            "direct imported runtime calls should not route through molt_guarded_call:\n{clif}"
        );
        assert!(
            !clif.contains("explicit_slot"),
            "direct imported runtime calls should not spill args for the guarded-call wrapper:\n{clif}"
        );
    }

    #[test]
    fn nested_exception_raise_if_does_not_synthesize_zero_predecessors() {
        let clif = compile_function_to_clif_text(
            vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        value: Some(0),
                        out: Some("flag".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["flag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "else".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".to_string(),
                        out: Some("exc".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("nonev".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".to_string(),
                        args: Some(vec!["exc".to_string(), "nonev".to_string()]),
                        out: Some("is_none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "not".to_string(),
                        args: Some(vec!["is_none".to_string()]),
                        out: Some("has_exc".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["has_exc".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_clear".to_string(),
                        out: Some("none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".to_string(),
                        args: Some(vec!["exc".to_string()]),
                        out: Some("none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "molt_main",
        );

        let suspicious: Vec<&str> = clif
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("jump block") && line.contains(" = 0"))
            .collect();

        assert!(
            suspicious.is_empty(),
            "nested exception raise CFG synthesized zero-valued predecessors:\n{}\n\nCLIF:\n{}",
            suspicious.join("\n"),
            clif
        );
    }

    #[test]
    fn rewrite_phi_to_store_load_rewrites_merge_phi() {
        let mut ops = vec![
            OpIR {
                kind: "const_bool".to_string(),
                value: Some(1),
                out: Some("cond".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".to_string(),
                args: Some(vec!["cond".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(1),
                out: Some("then_val".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(2),
                out: Some("else_val".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "phi".to_string(),
                out: Some("merged".to_string()),
                args: Some(vec!["then_val".to_string(), "else_val".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("merged".to_string()),
                ..OpIR::default()
            },
        ];

        rewrite_phi_to_store_load(&mut ops);

        assert!(
            ops.iter().all(|op| op.kind != "phi"),
            "phi should be eliminated: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "store_var"
                    && op.var.as_deref() == Some("_phi_merged")
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.len() == 1 && args[0] == "then_val")
            }),
            "then branch should store merged value"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "store_var"
                    && op.var.as_deref() == Some("_phi_merged")
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.len() == 1 && args[0] == "else_val")
            }),
            "else branch should store merged value"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "load_var"
                    && op.var.as_deref() == Some("_phi_merged")
                    && op.out.as_deref() == Some("merged")
            }),
            "merged phi should become load_var"
        );
    }

    #[test]
    fn fast_int_overflow_result_does_not_unbox_merged_bigint_result() {
        let clif = compile_function_to_clif_text(
            vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("base".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("exp".to_string()),
                        value: Some(63),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "pow".to_string(),
                        args: Some(vec!["base".to_string(), "exp".to_string()]),
                        out: Some("powv".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("one".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "sub".to_string(),
                        args: Some(vec!["powv".to_string(), "one".to_string()]),
                        out: Some("maxsize".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("maxsize".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "molt_main",
        );

        assert!(
            !clif.contains("block11(v43: i64):\n    v77 = iconst.i64 0x7fff_0000_0000_0000"),
            "merged overflow result must remain boxed until a real inline-int consumer proves otherwise:\n{clif}",
        );
    }

    #[test]
    fn drain_cleanup_entry_tracked_can_skip_named_value() {
        let mut names = vec!["callee".to_string(), "other".to_string()];
        let mut entry_vars = BTreeMap::new();
        let callee = Value::from_u32(11);
        let other = Value::from_u32(22);
        entry_vars.insert("callee".to_string(), callee);
        entry_vars.insert("other".to_string(), other);
        let last_use = BTreeMap::from([
            ("callee".to_string(), 5usize),
            ("other".to_string(), 5usize),
        ]);
        let alias_roots = BTreeMap::new();
        let mut already_decrefed = BTreeSet::new();

        let cleanup = drain_cleanup_entry_tracked(
            &mut names,
            &mut entry_vars,
            &last_use,
            &alias_roots,
            &mut already_decrefed,
            5,
            Some("callee"),
        );

        assert_eq!(cleanup, vec![other]);
        assert_eq!(names, vec!["callee".to_string()]);
        assert!(entry_vars.contains_key("callee"));
        assert!(!entry_vars.contains_key("other"));
    }
}
