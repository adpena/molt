use cranelift::codegen::ir::FuncRef;
use cranelift::codegen::isa;
use cranelift::codegen::Context;
use cranelift::prelude::*;
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

pub mod wasm;

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_WIDTH: u64 = 47;
const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
const INT_SHIFT: i64 = (64 - INT_WIDTH) as i64;
const GENERATOR_CONTROL_BYTES: i32 = 48;
const FUNC_DEFAULT_NONE: i64 = 1;
const FUNC_DEFAULT_DICT_POP: i64 = 2;
const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
const HEADER_SIZE_BYTES: i32 = 40;
const HEADER_STATE_OFFSET: i32 = -(HEADER_SIZE_BYTES - 16);
const HEADER_FLAGS_OFFSET: i32 = -8;
const HEADER_HAS_PTRS_FLAG: i64 = 1;

fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
}

fn box_float(val: f64) -> i64 {
    val.to_bits() as i64
}

fn pending_bits() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

fn unbox_int(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, INT_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, INT_SHIFT);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
}

fn is_int_tag(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, (QNAN | TAG_MASK) as i64);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_INT) as i64);
    let masked = builder.ins().band(val, mask);
    builder.ins().icmp(IntCC::Equal, masked, tag)
}

fn is_ptr_tag(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, (QNAN | TAG_MASK) as i64);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_PTR) as i64);
    let masked = builder.ins().band(val, mask);
    builder.ins().icmp(IntCC::Equal, masked, tag)
}

fn box_int_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, INT_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_INT) as i64);
    builder.ins().bor(tag, masked)
}

fn box_bool_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    let bool_val = builder.ins().select(val, one, zero);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_BOOL) as i64);
    builder.ins().bor(tag, bool_val)
}

fn unbox_ptr_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, POINTER_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, 16);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
}

fn box_ptr_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, POINTER_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_PTR) as i64);
    builder.ins().bor(tag, masked)
}

fn emit_maybe_ref_adjust(builder: &mut FunctionBuilder, val: Value, obj_ref_fn: FuncRef) {
    let current_block = builder
        .current_block()
        .expect("ref adjust requires an active block");
    let ptr_block = builder.create_block();
    let cont_block = builder.create_block();
    builder.insert_block_after(ptr_block, current_block);
    builder.insert_block_after(cont_block, ptr_block);

    let is_ptr = is_ptr_tag(builder, val);
    builder.ins().brif(is_ptr, ptr_block, &[], cont_block, &[]);

    builder.switch_to_block(ptr_block);
    builder.seal_block(ptr_block);
    builder.ins().call(obj_ref_fn, &[val]);
    builder.ins().jump(cont_block, &[]);

    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
}

fn emit_mark_has_ptrs(builder: &mut FunctionBuilder, obj_ptr: Value) {
    let header_ptr = builder
        .ins()
        .iadd_imm(obj_ptr, i64::from(HEADER_FLAGS_OFFSET));
    let flags = builder
        .ins()
        .load(types::I64, MemFlags::new(), header_ptr, 0);
    let mask = builder.ins().iconst(types::I64, HEADER_HAS_PTRS_FLAG);
    let new_flags = builder.ins().bor(flags, mask);
    builder
        .ins()
        .store(MemFlags::new(), new_flags, header_ptr, 0);
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SimpleIR {
    pub functions: Vec<FunctionIR>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FunctionIR {
    pub name: String,
    pub params: Vec<String>,
    pub ops: Vec<OpIR>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OpIR {
    pub kind: String,
    pub value: Option<i64>,
    pub f_value: Option<f64>,
    pub s_value: Option<String>,
    pub bytes: Option<Vec<u8>>,
    pub var: Option<String>,
    pub args: Option<Vec<String>>,
    pub out: Option<String>,
    #[serde(default)]
    pub fast_int: Option<bool>,
}

#[derive(Clone)]
struct TrackedValue {
    name: String,
    value: Value,
}

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

fn should_dump_ir() -> Option<DumpIrConfig> {
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

fn dump_ir_matches(config: &DumpIrConfig, func_name: &str) -> bool {
    let Some(filter) = config.filter.as_ref() else {
        return true;
    };
    if filter == "1" || filter.eq_ignore_ascii_case("all") {
        return true;
    }
    func_name == filter || func_name.contains(filter)
}

fn dump_ir_ops(func_ir: &FunctionIR, mode: &str) {
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
}

pub(crate) fn elide_dead_struct_allocs(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_STRUCT_ELIDE").is_ok() {
        return;
    }
    let mut remove = vec![false; func_ir.ops.len()];
    let alloc_kinds = ["alloc_class", "alloc_class_trusted", "alloc_class_static"];
    let allowed_use_kinds = [
        "store",
        "store_init",
        "guarded_field_set",
        "guarded_field_init",
        "object_set_class",
    ];

    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !alloc_kinds.contains(&op.kind.as_str()) {
            continue;
        }
        let Some(out_name) = op.out.as_ref() else {
            continue;
        };
        let mut allowed = true;
        let mut uses = Vec::new();
        for (use_idx, use_op) in func_ir.ops.iter().enumerate() {
            let Some(args) = use_op.args.as_ref() else {
                continue;
            };
            for (pos, arg) in args.iter().enumerate() {
                if arg != out_name {
                    continue;
                }
                if pos != 0 || !allowed_use_kinds.contains(&use_op.kind.as_str()) {
                    allowed = false;
                    break;
                }
                uses.push(use_idx);
            }
            if !allowed {
                break;
            }
        }
        if allowed {
            remove[idx] = true;
            for use_idx in uses {
                remove[use_idx] = true;
            }
        }
    }

    if remove.iter().any(|&flag| flag) {
        let mut new_ops = Vec::with_capacity(func_ir.ops.len());
        for (idx, op) in func_ir.ops.iter().enumerate() {
            if !remove[idx] {
                new_ops.push(op.clone());
            }
        }
        func_ir.ops = new_ops;
    }
}

fn compute_last_use(ops: &[OpIR]) -> HashMap<String, usize> {
    let mut last_use = HashMap::new();
    for (idx, op) in ops.iter().enumerate() {
        if let Some(args) = &op.args {
            for name in args {
                last_use.insert(name.clone(), idx);
            }
        }
        if let Some(var) = &op.var {
            last_use.insert(var.clone(), idx);
        }
    }
    last_use
}

fn drain_cleanup_tracked(
    names: &mut Vec<TrackedValue>,
    last_use: &HashMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<TrackedValue> {
    let mut cleanup = Vec::new();
    names.retain(|tracked| {
        if skip == Some(tracked.name.as_str()) {
            return true;
        }
        let last = last_use.get(&tracked.name).copied().unwrap_or(op_idx);
        if last <= op_idx {
            cleanup.push(tracked.clone());
            return false;
        }
        true
    });
    cleanup
}

fn collect_cleanup_tracked(
    names: &[TrackedValue],
    last_use: &HashMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<TrackedValue> {
    names
        .iter()
        .filter(|tracked| skip != Some(tracked.name.as_str()))
        .filter(|tracked| last_use.get(&tracked.name).copied().unwrap_or(op_idx) <= op_idx)
        .cloned()
        .collect()
}

pub struct SimpleBackend {
    module: ObjectModule,
    ctx: Context,
}

struct IfFrame {
    else_block: Block,
    merge_block: Block,
    has_else: bool,
    then_exit: Option<(Block, bool)>,
    else_exit: Option<(Block, bool)>,
    then_terminal: bool,
    else_terminal: bool,
    phi_ops: Option<Vec<(String, String, String)>>,
    vars_snapshot: HashMap<String, Value>,
    vars_then: Option<HashMap<String, Value>>,
}

struct LoopFrame {
    loop_block: Block,
    body_block: Block,
    after_block: Block,
    index_name: Option<String>,
    next_index: Option<Value>,
}

impl Default for SimpleBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleBackend {
    pub fn new() -> Self {
        Self::new_with_target(None)
    }

    pub fn new_with_target(target: Option<&str>) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        let isa_builder = if let Some(triple) = target {
            isa::lookup_by_name(triple).unwrap_or_else(|msg| {
                panic!("target {} is not supported: {}", triple, msg);
            })
        } else {
            cranelift_native::builder().unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        };
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let builder = ObjectBuilder::new(
            isa,
            "molt_output",
            cranelift_module::default_libcall_names(),
        )
        .unwrap();
        let module = ObjectModule::new(builder);
        let ctx = module.make_context();

        Self { module, ctx }
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        let mut ir = ir;
        for func_ir in &mut ir.functions {
            elide_dead_struct_allocs(func_ir);
        }
        for func_ir in ir.functions {
            self.compile_func(func_ir);
        }
        let product = self.module.finish();
        product.emit().unwrap()
    }

    fn compile_func(&mut self, func_ir: FunctionIR) {
        let mut builder_ctx = FunctionBuilderContext::new();
        self.module.clear_context(&mut self.ctx);

        let has_ret = func_ir
            .ops
            .iter()
            .any(|op| op.kind == "ret" || op.kind == "ret_void");
        if has_ret {
            self.ctx
                .func
                .signature
                .returns
                .push(AbiParam::new(types::I64));
        }
        for _ in &func_ir.params {
            self.ctx
                .func
                .signature
                .params
                .push(AbiParam::new(types::I64));
        }

        let param_types: Vec<types::Type> = self
            .ctx
            .func
            .signature
            .params
            .iter()
            .map(|p| p.value_type)
            .collect();
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut builder_ctx);

        let stateful = func_ir.ops.iter().any(|op| {
            matches!(
                op.kind.as_str(),
                "state_switch"
                    | "state_transition"
                    | "state_yield"
                    | "chan_send_yield"
                    | "chan_recv_yield"
            )
        });

        let mut vars = HashMap::new();
        let mut tracked_vars = Vec::new();
        let mut tracked_obj_vars = Vec::new();
        let mut state_blocks = HashMap::new();
        let mut resume_states: HashSet<i64> = HashSet::new();
        let mut is_block_filled = false;
        let mut if_stack: Vec<IfFrame> = Vec::new();
        let mut loop_stack: Vec<LoopFrame> = Vec::new();
        let mut loop_depth: i32 = 0;
        let mut block_tracked_obj: HashMap<Block, Vec<TrackedValue>> = HashMap::new();
        let mut block_tracked_ptr: HashMap<Block, Vec<TrackedValue>> = HashMap::new();
        let last_use = compute_last_use(&func_ir.ops);

        let entry_block = builder.create_block();
        let master_return_block = builder.create_block();
        if has_ret {
            builder.append_block_param(master_return_block, types::I64);
        }

        builder.switch_to_block(entry_block);

        let mut dec_ref_sig = self.module.make_signature();
        dec_ref_sig.params.push(AbiParam::new(types::I64));
        let dec_ref_callee = self
            .module
            .declare_function("molt_dec_ref", Linkage::Import, &dec_ref_sig)
            .unwrap();
        let local_dec_ref = self
            .module
            .declare_func_in_func(dec_ref_callee, builder.func);

        let mut dec_ref_obj_sig = self.module.make_signature();
        dec_ref_obj_sig.params.push(AbiParam::new(types::I64));
        let dec_ref_obj_callee = self
            .module
            .declare_function("molt_dec_ref_obj", Linkage::Import, &dec_ref_obj_sig)
            .unwrap();
        let local_dec_ref_obj = self
            .module
            .declare_func_in_func(dec_ref_obj_callee, builder.func);

        let mut inc_ref_obj_sig = self.module.make_signature();
        inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
        let inc_ref_obj_callee = self
            .module
            .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
            .unwrap();
        let local_inc_ref_obj = self
            .module
            .declare_func_in_func(inc_ref_obj_callee, builder.func);

        let profile_struct_sig = self.module.make_signature();
        let profile_struct_callee = self
            .module
            .declare_function(
                "molt_profile_struct_field_store",
                Linkage::Import,
                &profile_struct_sig,
            )
            .unwrap();
        let local_profile_struct = self
            .module
            .declare_func_in_func(profile_struct_callee, builder.func);

        let mut profile_enabled_sig = self.module.make_signature();
        profile_enabled_sig.returns.push(AbiParam::new(types::I64));
        let profile_enabled_callee = self
            .module
            .declare_function(
                "molt_profile_enabled",
                Linkage::Import,
                &profile_enabled_sig,
            )
            .unwrap();
        let local_profile_enabled = self
            .module
            .declare_func_in_func(profile_enabled_callee, builder.func);

        for (i, ty) in param_types.iter().enumerate() {
            let val = builder.append_block_param(entry_block, *ty);

            let name = &func_ir.params[i];

            vars.insert(name.clone(), val);
        }

        if stateful {
            if let Some(self_ptr) = vars.get("self").copied() {
                let self_bits = box_ptr_value(&mut builder, self_ptr);
                vars.insert("self".to_string(), self_bits);
            }
        }

        let profile_enabled_val = {
            let call = builder.ins().call(local_profile_enabled, &[]);
            builder.inst_results(call)[0]
        };

        builder.seal_block(entry_block);

        // 1. Pre-pass: discover states and create blocks
        for op in &func_ir.ops {
            let state_id = if op.kind == "state_transition"
                || op.kind == "state_yield"
                || op.kind == "chan_send_yield"
                || op.kind == "chan_recv_yield"
                || op.kind == "label"
                || op.kind == "state_label"
            {
                op.value.unwrap()
            } else {
                continue;
            };
            state_blocks
                .entry(state_id)
                .or_insert_with(|| builder.create_block());
            if op.kind == "state_yield" || op.kind == "state_label" {
                resume_states.insert(state_id);
            }
        }

        // 2. Implementation
        let ops = &func_ir.ops;
        let mut skip_ops: HashSet<usize> = HashSet::new();
        for op_idx in 0..ops.len() {
            if skip_ops.contains(&op_idx) {
                continue;
            }
            let op = ops[op_idx].clone();
            if is_block_filled {
                if op.kind == "if" {
                    let mut depth = 0usize;
                    let mut scan = op_idx + 1;
                    let mut end_if_idx = None;
                    while scan < ops.len() {
                        match ops[scan].kind.as_str() {
                            "if" => depth += 1,
                            "end_if" => {
                                if depth == 0 {
                                    end_if_idx = Some(scan);
                                    break;
                                }
                                depth -= 1;
                            }
                            _ => {}
                        }
                        scan += 1;
                    }
                    if let Some(end_if_idx) = end_if_idx {
                        for idx in op_idx..=end_if_idx {
                            skip_ops.insert(idx);
                        }
                        let mut phi_idx = end_if_idx + 1;
                        while phi_idx < ops.len() {
                            if ops[phi_idx].kind != "phi" {
                                break;
                            }
                            skip_ops.insert(phi_idx);
                            phi_idx += 1;
                        }
                        continue;
                    }
                }
                match op.kind.as_str() {
                    "label" | "state_label" | "else" | "end_if" | "loop_end" => {}
                    _ => continue,
                }
            }
            let out_name = op.out.clone();
            let mut output_is_ptr = false;

            match op.kind.as_str() {
                "const" => {
                    let val = op.value.unwrap();
                    let boxed = box_int(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    vars.insert(op.out.unwrap(), iconst);
                }
                "const_bigint" => {
                    let s = op.s_value.as_ref().expect("BigInt string not found");
                    let out_name = op.out.unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("bigint_{}_{}", func_ir.name, out_name),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(s.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, s.len() as i64);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bigint_from_str", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[ptr, len]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(out_name, res);
                }
                "const_bool" => {
                    let val = op.value.unwrap();
                    let boxed = box_bool(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    vars.insert(op.out.unwrap(), iconst);
                }
                "const_none" => {
                    let iconst = builder.ins().iconst(types::I64, box_none());
                    vars.insert(op.out.unwrap(), iconst);
                }
                "const_not_implemented" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_not_implemented", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "const_float" => {
                    let val = op.f_value.expect("Float value not found");
                    let boxed = box_float(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    vars.insert(op.out.unwrap(), iconst);
                }
                "const_str" => {
                    let s = op.s_value.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("str_{}_{}", func_ir.name, out_name),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(s.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, s.len() as i64);

                    vars.insert(format!("{}_ptr", out_name), ptr);
                    vars.insert(format!("{}_len", out_name), len);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // bytes ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                    let callee = self
                        .module
                        .declare_function("molt_string_from_bytes", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
                    let boxed = builder.ins().load(types::I64, MemFlags::new(), out_ptr, 0);

                    vars.insert(out_name, boxed);
                }
                "const_bytes" => {
                    let bytes = op.bytes.as_ref().expect("Bytes not found");
                    let out_name = op.out.unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("bytes_{}_{}", func_ir.name, out_name),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(bytes.clone().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, bytes.len() as i64);

                    vars.insert(format!("{}_ptr", out_name), ptr);
                    vars.insert(format!("{}_len", out_name), len);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // bytes ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                    let callee = self
                        .module
                        .declare_function("molt_bytes_from_bytes", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
                    let boxed = builder.ins().load(types::I64, MemFlags::new(), out_ptr, 0);

                    vars.insert(out_name, boxed);
                }
                "add" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        box_int_value(&mut builder, sum)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_add", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "inplace_add" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        box_int_value(&mut builder, sum)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_add", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_sum_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_sum_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_sum_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_sum_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_prod_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_prod_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_prod_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_prod_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_min_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_min_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_min_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_min_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_max_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_max_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_max_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "vec_max_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = vars.get(&args[0]).expect("Seq arg not found");
                    let acc = vars.get(&args[1]).expect("Acc arg not found");
                    let start = vars.get(&args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "sub" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("LHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let rhs = vars.get(&args[1]).unwrap_or_else(|| {
                        panic!("RHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        box_int_value(&mut builder, diff)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, diff);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_sub", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "inplace_sub" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("LHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let rhs = vars.get(&args[1]).unwrap_or_else(|| {
                        panic!("RHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        box_int_value(&mut builder, diff)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, diff);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_sub", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "mul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        box_int_value(&mut builder, prod)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_mul", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "inplace_mul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        box_int_value(&mut builder, prod)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_mul", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "bit_or" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bit_or", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "inplace_bit_or" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_inplace_bit_or", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bit_and" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bit_and", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "inplace_bit_and" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_inplace_bit_and", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bit_xor" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bit_xor", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "inplace_bit_xor" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_inplace_bit_xor", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "lshift" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_lshift", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "rshift" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_rshift", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "matmul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_matmul", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "div" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_div", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "floordiv" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_floordiv", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "mod" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_mod", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "pow" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_pow", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "pow_mod" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let modulus = vars.get(&args[2]).expect("Mod not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_pow_mod", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs, *modulus]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "round" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Round arg not found");
                    let ndigits = vars.get(&args[1]).expect("Round ndigits not found");
                    let has_ndigits = vars.get(&args[2]).expect("Round ndigits flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_round", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*val, *ndigits, *has_ndigits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "trunc" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Trunc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_trunc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "len" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Len arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_len", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "id" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Id arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_id", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "ord" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Ord arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_ord", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "chr" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Chr arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "callargs_new" => {
                    let out_name = op.out.unwrap();
                    let zero = builder.ins().iconst(types::I64, 0);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[zero, zero]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(out_name, res);
                }
                "list_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_list_builder_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let builder_ptr = builder.inst_results(new_call)[0];

                    let mut append_sig = self.module.make_signature();
                    append_sig.params.push(AbiParam::new(types::I64));
                    append_sig.params.push(AbiParam::new(types::I64));
                    let append_callee = self
                        .module
                        .declare_function("molt_list_builder_append", Linkage::Import, &append_sig)
                        .unwrap();
                    let append_local = self
                        .module
                        .declare_func_in_func(append_callee, builder.func);
                    for name in args {
                        let val = vars.get(name).unwrap_or_else(|| {
                            panic!("List elem not found in {} op {}", func_ir.name, op_idx)
                        });
                        builder.ins().call(append_local, &[builder_ptr, *val]);
                    }

                    let mut finish_sig = self.module.make_signature();
                    finish_sig.params.push(AbiParam::new(types::I64));
                    finish_sig.returns.push(AbiParam::new(types::I64));
                    let finish_callee = self
                        .module
                        .declare_function("molt_list_builder_finish", Linkage::Import, &finish_sig)
                        .unwrap();
                    let finish_local = self
                        .module
                        .declare_func_in_func(finish_callee, builder.func);
                    let finish_call = builder.ins().call(finish_local, &[builder_ptr]);
                    let list_bits = builder.inst_results(finish_call)[0];
                    vars.insert(out_name, list_bits);
                }
                "callargs_push_pos" => {
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr = vars.get(&args[0]).expect("Callargs builder not found");
                    let val = vars.get(&args[1]).expect("Callargs value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *val]);
                }
                "callargs_push_kw" => {
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr = vars.get(&args[0]).expect("Callargs builder not found");
                    let name = vars.get(&args[1]).expect("Callargs name not found");
                    let val = vars.get(&args[2]).expect("Callargs value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_push_kw", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*builder_ptr, *name, *val]);
                }
                "callargs_expand_star" => {
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr = vars.get(&args[0]).expect("Callargs builder not found");
                    let iterable = vars.get(&args[1]).expect("Callargs iterable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_expand_star", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *iterable]);
                }
                "callargs_expand_kwstar" => {
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr = vars.get(&args[0]).expect("Callargs builder not found");
                    let mapping = vars.get(&args[1]).expect("Callargs mapping not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_expand_kwstar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *mapping]);
                }
                "range_new" => {
                    let args = op.args.as_ref().unwrap();
                    let start = vars.get(&args[0]).expect("Range start not found");
                    let stop = vars.get(&args[1]).expect("Range stop not found");
                    let step = vars.get(&args[2]).expect("Range step not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_range_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "tuple_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_list_builder_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let builder_ptr = builder.inst_results(new_call)[0];

                    let mut append_sig = self.module.make_signature();
                    append_sig.params.push(AbiParam::new(types::I64));
                    append_sig.params.push(AbiParam::new(types::I64));
                    let append_callee = self
                        .module
                        .declare_function("molt_list_builder_append", Linkage::Import, &append_sig)
                        .unwrap();
                    let append_local = self
                        .module
                        .declare_func_in_func(append_callee, builder.func);
                    for name in args {
                        let val = vars.get(name).expect("Tuple elem not found");
                        builder.ins().call(append_local, &[builder_ptr, *val]);
                    }

                    let mut finish_sig = self.module.make_signature();
                    finish_sig.params.push(AbiParam::new(types::I64));
                    finish_sig.returns.push(AbiParam::new(types::I64));
                    let finish_callee = self
                        .module
                        .declare_function("molt_tuple_builder_finish", Linkage::Import, &finish_sig)
                        .unwrap();
                    let finish_local = self
                        .module
                        .declare_func_in_func(finish_callee, builder.func);
                    let finish_call = builder.ins().call(finish_local, &[builder_ptr]);
                    let tuple_bits = builder.inst_results(finish_call)[0];
                    vars.insert(out_name, tuple_bits);
                }
                "list_append" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let val = vars.get(&args[1]).expect("List append value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_append", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_pop" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let idx = vars.get(&args[1]).expect("List pop index not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *idx]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_extend" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let other = vars.get(&args[1]).expect("List extend iterable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_extend", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *other]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_insert" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let idx = vars.get(&args[1]).expect("List insert index not found");
                    let val = vars.get(&args[2]).expect("List insert value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_insert", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_remove" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let val = vars.get(&args[1]).expect("List remove value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_remove", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_clear" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_copy" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_copy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_reverse" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_reverse", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_count" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let val = vars.get(&args[1]).expect("List count value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_index" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let val = vars.get(&args[1]).expect("List index value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "list_index_range" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("List not found");
                    let val = vars.get(&args[1]).expect("List index value not found");
                    let start = vars.get(&args[2]).expect("List index start not found");
                    let stop = vars.get(&args[3]).expect("List index stop not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_index_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*list, *val, *start, *stop]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "tuple_from_list" => {
                    let args = op.args.as_ref().unwrap();
                    let list = vars.get(&args[0]).expect("Tuple source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_tuple_from_list", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, (args.len() / 2) as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_dict_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let dict_bits = builder.inst_results(new_call)[0];

                    let mut set_sig = self.module.make_signature();
                    set_sig.params.push(AbiParam::new(types::I64));
                    set_sig.params.push(AbiParam::new(types::I64));
                    set_sig.params.push(AbiParam::new(types::I64));
                    set_sig.returns.push(AbiParam::new(types::I64));
                    let set_callee = self
                        .module
                        .declare_function("molt_dict_set", Linkage::Import, &set_sig)
                        .unwrap();
                    let set_local = self.module.declare_func_in_func(set_callee, builder.func);
                    let mut current = dict_bits;
                    for pair in args.chunks(2) {
                        let key = vars.get(&pair[0]).expect("Dict key not found");
                        let val = vars.get(&pair[1]).expect("Dict val not found");
                        let set_call = builder.ins().call(set_local, &[current, *key, *val]);
                        current = builder.inst_results(set_call)[0];
                    }
                    vars.insert(out_name, current);
                }
                "dict_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Dict source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_set_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let set_bits = builder.inst_results(new_call)[0];

                    if !args.is_empty() {
                        let mut add_sig = self.module.make_signature();
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.returns.push(AbiParam::new(types::I64));
                        let add_callee = self
                            .module
                            .declare_function("molt_set_add", Linkage::Import, &add_sig)
                            .unwrap();
                        let add_local = self.module.declare_func_in_func(add_callee, builder.func);
                        for name in args {
                            let val = vars.get(name).unwrap_or_else(|| {
                                panic!("Set elem not found in {} op {}", func_ir.name, op_idx)
                            });
                            builder.ins().call(add_local, &[set_bits, *val]);
                        }
                    }

                    vars.insert(out_name, set_bits);
                }
                "frozenset_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_frozenset_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let set_bits = builder.inst_results(new_call)[0];

                    if !args.is_empty() {
                        let mut add_sig = self.module.make_signature();
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.returns.push(AbiParam::new(types::I64));
                        let add_callee = self
                            .module
                            .declare_function("molt_frozenset_add", Linkage::Import, &add_sig)
                            .unwrap();
                        let add_local = self.module.declare_func_in_func(add_callee, builder.func);
                        for name in args {
                            let val = vars.get(name).unwrap_or_else(|| {
                                panic!("Frozenset elem not found in {} op {}", func_ir.name, op_idx)
                            });
                            builder.ins().call(add_local, &[set_bits, *val]);
                        }
                    }

                    vars.insert(out_name, set_bits);
                }
                "dict_get" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let key = vars.get(&args[1]).expect("Dict key not found");
                    let default = vars.get(&args[2]).expect("Dict default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_pop" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let key = vars.get(&args[1]).expect("Dict key not found");
                    let default = vars.get(&args[2]).expect("Dict default not found");
                    let has_default = vars.get(&args[3]).expect("Dict default flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict, *key, *default, *has_default]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_setdefault" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let key = vars.get(&args[1]).expect("Dict key not found");
                    let default = vars.get(&args[2]).expect("Dict default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_setdefault", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_update" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let other = vars.get(&args[1]).expect("Dict update iterable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *other]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_clear" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_copy" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_copy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_popitem" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_popitem", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_update_kwstar" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let other = vars.get(&args[1]).expect("Dict update mapping not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_update_kwstar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *other]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_add" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let key_bits = vars.get(&args[1]).expect("Set key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_add", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "frozenset_add" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Frozenset not found");
                    let key_bits = vars.get(&args[1]).expect("Frozenset key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_frozenset_add", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_discard" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let key_bits = vars.get(&args[1]).expect("Set key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_discard", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_remove" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let key_bits = vars.get(&args[1]).expect("Set key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_remove", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_pop" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let other_bits = vars.get(&args[1]).expect("Set update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_intersection_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let other_bits = vars
                        .get(&args[1])
                        .expect("Set intersection update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_intersection_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_difference_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let other_bits = vars
                        .get(&args[1])
                        .expect("Set difference update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_difference_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_symdiff_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = vars.get(&args[0]).expect("Set not found");
                    let other_bits = vars
                        .get(&args[1])
                        .expect("Set symdiff update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_symdiff_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_keys" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_keys", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_values" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_values", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dict_items" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = vars.get(&args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_items", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "tuple_count" => {
                    let args = op.args.as_ref().unwrap();
                    let tuple = vars.get(&args[0]).expect("Tuple not found");
                    let val = vars.get(&args[1]).expect("Tuple count value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_tuple_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tuple, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "tuple_index" => {
                    let args = op.args.as_ref().unwrap();
                    let tuple = vars.get(&args[0]).expect("Tuple not found");
                    let val = vars.get(&args[1]).expect("Tuple index value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_tuple_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tuple, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "iter" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Iter source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_iter_checked", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "enumerate" => {
                    let args = op.args.as_ref().unwrap();
                    let iterable = vars.get(&args[0]).expect("Enumerate iterable not found");
                    let start = vars.get(&args[1]).expect("Enumerate start not found");
                    let has_start = vars.get(&args[2]).expect("Enumerate has_start not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_enumerate", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*iterable, *start, *has_start]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "aiter" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Async iter source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_aiter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "iter_next" => {
                    let args = op.args.as_ref().unwrap();
                    let iter = vars.get(&args[0]).expect("Iter not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_iter_next", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*iter]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "anext" => {
                    let args = op.args.as_ref().unwrap();
                    let iter = vars.get(&args[0]).expect("Async iter not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_anext", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*iter]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "gen_send" => {
                    let args = op.args.as_ref().unwrap();
                    let gen = vars.get(&args[0]).expect("Generator not found");
                    let val = vars.get(&args[1]).expect("Send value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_generator_send", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "gen_throw" => {
                    let args = op.args.as_ref().unwrap();
                    let gen = vars.get(&args[0]).expect("Generator not found");
                    let val = vars.get(&args[1]).expect("Throw value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_generator_throw", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "gen_close" => {
                    let args = op.args.as_ref().unwrap();
                    let gen = vars.get(&args[0]).expect("Generator not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_generator_close", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "is_generator" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Obj not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_generator", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "is_bound_method" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Obj not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_bound_method", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "is_callable" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Obj not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_callable", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "index" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Obj not found");
                    let idx = vars.get(&args[1]).expect("Index not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "store_index" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Obj not found in {} op {}", func_ir.name, op_idx)
                    });
                    let idx = vars.get(&args[1]).unwrap_or_else(|| {
                        panic!("Index not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val = vars.get(&args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_store_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "del_index" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Obj not found in {} op {}", func_ir.name, op_idx)
                    });
                    let idx = vars.get(&args[1]).unwrap_or_else(|| {
                        panic!("Index not found in {} op {}", func_ir.name, op_idx)
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "slice" => {
                    let args = op.args.as_ref().unwrap();
                    let target = vars.get(&args[0]).expect("Slice target not found");
                    let start = vars.get(&args[1]).expect("Slice start not found");
                    let end = vars.get(&args[2]).expect("Slice end not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*target, *start, *end]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "slice_new" => {
                    let args = op.args.as_ref().unwrap();
                    let start = vars.get(&args[0]).expect("Slice start not found");
                    let stop = vars.get(&args[1]).expect("Slice stop not found");
                    let step = vars.get(&args[2]).expect("Slice step not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_slice_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_find" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Find haystack not found");
                    let needle = vars.get(&args[1]).expect("Find needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_find", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_find_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Find haystack not found");
                    let needle = vars.get(&args[1]).expect("Find needle not found");
                    let start = vars.get(&args[2]).expect("Find start not found");
                    let end = vars.get(&args[3]).expect("Find end not found");
                    let has_start = vars.get(&args[4]).expect("Find has_start not found");
                    let has_end = vars.get(&args[5]).expect("Find has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_find_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_find" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Find haystack not found");
                    let needle = vars.get(&args[1]).expect("Find needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_find", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_find_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Find haystack not found");
                    let needle = vars.get(&args[1]).expect("Find needle not found");
                    let start = vars.get(&args[2]).expect("Find start not found");
                    let end = vars.get(&args[3]).expect("Find end not found");
                    let has_start = vars.get(&args[4]).expect("Find has_start not found");
                    let has_end = vars.get(&args[5]).expect("Find has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_find_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_find" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Find haystack not found");
                    let needle = vars.get(&args[1]).expect("Find needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_find", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_find_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Find haystack not found");
                    let needle = vars.get(&args[1]).expect("Find needle not found");
                    let start = vars.get(&args[2]).expect("Find start not found");
                    let end = vars.get(&args[3]).expect("Find end not found");
                    let has_start = vars.get(&args[4]).expect("Find has_start not found");
                    let has_end = vars.get(&args[5]).expect("Find has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_find_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_format" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Format value not found");
                    let spec = vars.get(&args[1]).expect("Format spec not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_format", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *spec]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_startswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Startswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Startswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_startswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_startswith_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Startswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Startswith needle not found");
                    let start = vars.get(&args[2]).expect("Startswith start not found");
                    let end = vars.get(&args[3]).expect("Startswith end not found");
                    let has_start = vars.get(&args[4]).expect("Startswith has_start not found");
                    let has_end = vars.get(&args[5]).expect("Startswith has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_startswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_startswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Startswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Startswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_startswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_startswith_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Startswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Startswith needle not found");
                    let start = vars.get(&args[2]).expect("Startswith start not found");
                    let end = vars.get(&args[3]).expect("Startswith end not found");
                    let has_start = vars.get(&args[4]).expect("Startswith has_start not found");
                    let has_end = vars.get(&args[5]).expect("Startswith has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_startswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_startswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Startswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Startswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_startswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_startswith_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Startswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Startswith needle not found");
                    let start = vars.get(&args[2]).expect("Startswith start not found");
                    let end = vars.get(&args[3]).expect("Startswith end not found");
                    let has_start = vars.get(&args[4]).expect("Startswith has_start not found");
                    let has_end = vars.get(&args[5]).expect("Startswith has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_startswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_endswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Endswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Endswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_endswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_endswith_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Endswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Endswith needle not found");
                    let start = vars.get(&args[2]).expect("Endswith start not found");
                    let end = vars.get(&args[3]).expect("Endswith end not found");
                    let has_start = vars.get(&args[4]).expect("Endswith has_start not found");
                    let has_end = vars.get(&args[5]).expect("Endswith has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_endswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_endswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Endswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Endswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_endswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_endswith_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Endswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Endswith needle not found");
                    let start = vars.get(&args[2]).expect("Endswith start not found");
                    let end = vars.get(&args[3]).expect("Endswith end not found");
                    let has_start = vars.get(&args[4]).expect("Endswith has_start not found");
                    let has_end = vars.get(&args[5]).expect("Endswith has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_endswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_endswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Endswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Endswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_endswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_endswith_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Endswith haystack not found");
                    let needle = vars.get(&args[1]).expect("Endswith needle not found");
                    let start = vars.get(&args[2]).expect("Endswith start not found");
                    let end = vars.get(&args[3]).expect("Endswith end not found");
                    let has_start = vars.get(&args[4]).expect("Endswith has_start not found");
                    let has_end = vars.get(&args[5]).expect("Endswith has_end not found");
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_endswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_count" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Count haystack not found");
                    let needle = vars.get(&args[1]).expect("Count needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_count" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Count haystack not found");
                    let needle = vars.get(&args[1]).expect("Count needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_count" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Count haystack not found");
                    let needle = vars.get(&args[1]).expect("Count needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_count_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Count haystack not found");
                    let needle = vars.get(&args[1]).expect("Count needle not found");
                    let start = vars.get(&args[2]).expect("Count start not found");
                    let end = vars.get(&args[3]).expect("Count end not found");
                    let has_start = vars.get(&args[4]).expect("Count has_start not found");
                    let has_end = vars.get(&args[5]).expect("Count has_end not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_count_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_count_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Count haystack not found");
                    let needle = vars.get(&args[1]).expect("Count needle not found");
                    let start = vars.get(&args[2]).expect("Count start not found");
                    let end = vars.get(&args[3]).expect("Count end not found");
                    let has_start = vars.get(&args[4]).expect("Count has_start not found");
                    let has_end = vars.get(&args[5]).expect("Count has_end not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_count_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_count_slice" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Count haystack not found");
                    let needle = vars.get(&args[1]).expect("Count needle not found");
                    let start = vars.get(&args[2]).expect("Count start not found");
                    let end = vars.get(&args[3]).expect("Count end not found");
                    let has_start = vars.get(&args[4]).expect("Count has_start not found");
                    let has_end = vars.get(&args[5]).expect("Count has_end not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_count_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "env_get" => {
                    let args = op.args.as_ref().unwrap();
                    let key = vars.get(&args[0]).expect("Env key not found");
                    let default = vars.get(&args[1]).expect("Env default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_env_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*key, *default]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_join" => {
                    let args = op.args.as_ref().unwrap();
                    let sep = vars.get(&args[0]).expect("Join separator not found");
                    let items = vars.get(&args[1]).expect("Join items not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_join", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*sep, *items]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_split" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Split haystack not found");
                    let needle = vars.get(&args[1]).expect("Split needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_split", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_split_max" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Split haystack not found");
                    let needle = vars.get(&args[1]).expect("Split needle not found");
                    let maxsplit = vars.get(&args[2]).expect("Split maxsplit not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_split_max", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_lower" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Lower string not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_lower", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_upper" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Upper string not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_upper", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_capitalize" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Capitalize string not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_capitalize", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_strip" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Strip string not found");
                    let chars = vars.get(&args[1]).expect("Strip chars not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_strip", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "string_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Replace haystack not found");
                    let needle = vars.get(&args[1]).expect("Replace needle not found");
                    let replacement = vars.get(&args[2]).expect("Replace replacement not found");
                    let count = vars.get(&args[3]).expect("Replace count not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_replace", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_split" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Split haystack not found");
                    let needle = vars.get(&args[1]).expect("Split needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_split", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_split_max" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Split haystack not found");
                    let needle = vars.get(&args[1]).expect("Split needle not found");
                    let maxsplit = vars.get(&args[2]).expect("Split maxsplit not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_split_max", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_split" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Split haystack not found");
                    let needle = vars.get(&args[1]).expect("Split needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_split", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_split_max" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Split haystack not found");
                    let needle = vars.get(&args[1]).expect("Split needle not found");
                    let maxsplit = vars.get(&args[2]).expect("Split maxsplit not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_split_max", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Replace haystack not found");
                    let needle = vars.get(&args[1]).expect("Replace needle not found");
                    let replacement = vars.get(&args[2]).expect("Replace replacement not found");
                    let count = vars.get(&args[3]).expect("Replace count not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_replace", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Replace haystack not found");
                    let needle = vars.get(&args[1]).expect("Replace needle not found");
                    let replacement = vars.get(&args[2]).expect("Replace replacement not found");
                    let count = vars.get(&args[3]).expect("Replace count not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_replace", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Bytes source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytes_from_str" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Bytes source not found");
                    let encoding = vars.get(&args[1]).expect("Bytes encoding not found");
                    let errors = vars.get(&args[2]).expect("Bytes errors not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_from_str", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*src, *encoding, *errors]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Bytearray source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_from_str" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Bytearray source not found");
                    let encoding = vars.get(&args[1]).expect("Bytearray encoding not found");
                    let errors = vars.get(&args[2]).expect("Bytearray errors not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_from_str", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*src, *encoding, *errors]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "float_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Float source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_float_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "int_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Int value not found");
                    let base = vars.get(&args[1]).expect("Int base not found");
                    let has_base = vars.get(&args[2]).expect("Int base flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_int_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *base, *has_base]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "intarray_from_seq" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Intarray source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_intarray_from_seq", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "memoryview_new" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Memoryview source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_memoryview_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "memoryview_tobytes" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Memoryview value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_memoryview_tobytes", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "memoryview_cast" => {
                    let args = op.args.as_ref().unwrap();
                    let view = vars.get(&args[0]).expect("Memoryview not found");
                    let format = vars.get(&args[1]).expect("Memoryview format not found");
                    let shape = vars.get(&args[2]).expect("Memoryview shape not found");
                    let has_shape = vars.get(&args[3]).expect("Memoryview shape flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_memoryview_cast", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*view, *format, *shape, *has_shape]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "buffer2d_new" => {
                    let args = op.args.as_ref().unwrap();
                    let rows = vars.get(&args[0]).expect("Buffer2D rows not found");
                    let cols = vars.get(&args[1]).expect("Buffer2D cols not found");
                    let init = vars.get(&args[2]).expect("Buffer2D init not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*rows, *cols, *init]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "buffer2d_get" => {
                    let args = op.args.as_ref().unwrap();
                    let buf = vars.get(&args[0]).expect("Buffer2D not found");
                    let row = vars.get(&args[1]).expect("Buffer2D row not found");
                    let col = vars.get(&args[2]).expect("Buffer2D col not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*buf, *row, *col]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "buffer2d_set" => {
                    let args = op.args.as_ref().unwrap();
                    let buf = vars.get(&args[0]).expect("Buffer2D not found");
                    let row = vars.get(&args[1]).expect("Buffer2D row not found");
                    let col = vars.get(&args[2]).expect("Buffer2D col not found");
                    let val = vars.get(&args[3]).expect("Buffer2D val not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*buf, *row, *col, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "buffer2d_matmul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("Buffer2D lhs not found");
                    let rhs = vars.get(&args[1]).expect("Buffer2D rhs not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_matmul", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "str_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Str source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_str_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "repr_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Repr source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_repr_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "ascii_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = vars.get(&args[0]).expect("Ascii source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_ascii_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dataclass_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name = vars.get(&args[0]).expect("Dataclass name not found");
                    let fields = vars.get(&args[1]).expect("Dataclass fields not found");
                    let values = vars.get(&args[2]).expect("Dataclass values not found");
                    let flags = vars.get(&args[3]).expect("Dataclass flags not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*name, *fields, *values, *flags]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dataclass_get" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Dataclass object not found");
                    let idx = vars.get(&args[1]).expect("Dataclass index not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dataclass_set" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Dataclass object not found");
                    let idx = vars.get(&args[1]).expect("Dataclass index not found");
                    let val = vars.get(&args[2]).expect("Dataclass value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "dataclass_set_class" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Dataclass object not found");
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_set_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "lt" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_lt", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "le" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_le", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "gt" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_gt", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "ge" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_ge", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "eq" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let lhs_is_int = is_int_tag(&mut builder, *lhs);
                        let rhs_is_int = is_int_tag(&mut builder, *rhs);
                        let both_int = builder.ins().band(lhs_is_int, rhs_is_int);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp);
                        builder.ins().jump(merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_eq", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        builder.ins().jump(merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    vars.insert(op.out.unwrap(), res);
                }
                "string_eq" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_eq", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "is" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "not" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_not", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "invert" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_invert", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "and" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let truthy = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let truthy_ref = self.module.declare_func_in_func(truthy, builder.func);
                    let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                    let rhs_call = builder.ins().call(truthy_ref, &[*rhs]);
                    let lhs_val = builder.inst_results(lhs_call)[0];
                    let rhs_val = builder.inst_results(rhs_call)[0];
                    let and_val = builder.ins().band(lhs_val, rhs_val);
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, and_val, 0);
                    let res = box_bool_value(&mut builder, cond);
                    vars.insert(op.out.unwrap(), res);
                }
                "or" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let truthy = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let truthy_ref = self.module.declare_func_in_func(truthy, builder.func);
                    let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                    let rhs_call = builder.ins().call(truthy_ref, &[*rhs]);
                    let lhs_val = builder.inst_results(lhs_call)[0];
                    let rhs_val = builder.inst_results(rhs_call)[0];
                    let or_val = builder.ins().bor(lhs_val, rhs_val);
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, or_val, 0);
                    let res = box_bool_value(&mut builder, cond);
                    vars.insert(op.out.unwrap(), res);
                }
                "contains" => {
                    let args = op.args.as_ref().unwrap();
                    let container = vars.get(&args[0]).expect("Container not found");
                    let item = vars.get(&args[1]).expect("Item not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_contains", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*container, *item]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "print" => {
                    let args = op.args.as_ref().unwrap();
                    let val = if let Some(val) = vars.get(&args[0]) {
                        *val
                    } else {
                        builder.ins().iconst(types::I64, box_none())
                    };

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_print_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[val]);
                }
                "print_newline" => {
                    let sig = self.module.make_signature();
                    let callee = self
                        .module
                        .declare_function("molt_print_newline", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[]);
                }
                "json_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    let ptr = vars
                        .get(&format!("{}_ptr", arg_name))
                        .or_else(|| vars.get(arg_name))
                        .expect("String ptr not found");
                    let len = vars
                        .get(&format!("{}_len", arg_name))
                        .expect("String len not found");

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status

                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                    let callee = self
                        .module
                        .declare_function("molt_json_parse_scalar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                    let res = builder.ins().load(types::I64, MemFlags::new(), out_ptr, 0);
                    vars.insert(op.out.unwrap(), res);
                }
                "msgpack_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    let ptr = vars
                        .get(&format!("{}_ptr", arg_name))
                        .or_else(|| vars.get(arg_name))
                        .expect("Bytes ptr not found");
                    let len = vars
                        .get(&format!("{}_len", arg_name))
                        .expect("Bytes len not found");

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status

                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                    let callee = self
                        .module
                        .declare_function("molt_msgpack_parse_scalar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                    let res = builder.ins().load(types::I64, MemFlags::new(), out_ptr, 0);
                    vars.insert(op.out.unwrap(), res);
                }
                "cbor_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    let ptr = vars
                        .get(&format!("{}_ptr", arg_name))
                        .or_else(|| vars.get(arg_name))
                        .expect("Bytes ptr not found");
                    let len = vars
                        .get(&format!("{}_len", arg_name))
                        .expect("Bytes len not found");

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status

                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                    let callee = self
                        .module
                        .declare_function("molt_cbor_parse_scalar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                    let res = builder.ins().load(types::I64, MemFlags::new(), out_ptr, 0);
                    vars.insert(op.out.unwrap(), res);
                }
                "block_on" => {
                    let args = op.args.as_ref().unwrap();
                    let task = vars.get(&args[0]).expect("Task not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // boxed task
                    sig.returns.push(AbiParam::new(types::I64));

                    let callee = self
                        .module
                        .declare_function("molt_block_on", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*task]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "state_switch" => {
                    let self_ptr = builder.block_params(entry_block)[0];
                    let state = builder.ins().load(
                        types::I64,
                        MemFlags::new(),
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );
                    let self_bits = box_ptr_value(&mut builder, self_ptr);
                    vars.insert("self".to_string(), self_bits);

                    let mut sorted_states: Vec<_> = resume_states.iter().copied().collect();
                    sorted_states.sort();

                    for id in sorted_states {
                        let block = state_blocks[&id];
                        let id_const = builder.ins().iconst(types::I64, id);
                        let is_state = builder.ins().icmp(IntCC::Equal, state, id_const);
                        let next_check = builder.create_block();
                        if let Some(current_block) = builder.current_block() {
                            builder.insert_block_after(next_check, current_block);
                        }
                        builder.ins().brif(is_state, block, &[], next_check, &[]);
                        builder.switch_to_block(next_check);
                        builder.seal_block(next_check);
                    }
                }
                "state_transition" => {
                    let args = op.args.as_ref().unwrap();
                    let future = vars.get(&args[0]).expect("Future not found");
                    let future_ptr = unbox_ptr_value(&mut builder, *future);
                    let (slot_bits, pending_state_bits) = if args.len() == 2 {
                        (None, *vars.get(&args[1]).expect("Pending state not found"))
                    } else {
                        (
                            Some(*vars.get(&args[1]).expect("Await slot not found")),
                            *vars.get(&args[2]).expect("Pending state not found"),
                        )
                    };
                    let next_state_id = op.value.unwrap();
                    let self_bits = *vars.get("self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits);
                    builder.ins().store(
                        MemFlags::new(),
                        pending_state_id,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));
                    let poll_callee = self
                        .module
                        .declare_function("molt_future_poll", Linkage::Import, &poll_sig)
                        .unwrap();
                    let local_poll = self.module.declare_func_in_func(poll_callee, builder.func);
                    let poll_call = builder.ins().call(local_poll, &[*future]);
                    let res = builder.inst_results(poll_call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let pending_path = builder.create_block();
                    let ready_path = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(ready_path, current_block);
                    }
                    builder
                        .ins()
                        .brif(is_pending, pending_path, &[], ready_path, &[]);

                    builder.switch_to_block(pending_path);
                    builder.seal_block(pending_path);
                    let mut sleep_sig = self.module.make_signature();
                    sleep_sig.params.push(AbiParam::new(types::I64));
                    sleep_sig.params.push(AbiParam::new(types::I64));
                    sleep_sig.returns.push(AbiParam::new(types::I64));
                    let sleep_callee = self
                        .module
                        .declare_function("molt_sleep_register", Linkage::Import, &sleep_sig)
                        .unwrap();
                    let local_sleep = self.module.declare_func_in_func(sleep_callee, builder.func);
                    builder.ins().call(local_sleep, &[self_ptr, future_ptr]);
                    builder.ins().jump(master_return_block, &[pending_const]);

                    builder.switch_to_block(ready_path);
                    builder.seal_block(ready_path);
                    if let Some(bits) = slot_bits {
                        let offset = unbox_int(&mut builder, bits);
                        let mut store_sig = self.module.make_signature();
                        store_sig.params.push(AbiParam::new(types::I64));
                        store_sig.params.push(AbiParam::new(types::I64));
                        store_sig.params.push(AbiParam::new(types::I64));
                        store_sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_closure_store", Linkage::Import, &store_sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        builder.ins().call(local_callee, &[self_ptr, offset, res]);
                    }
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, HEADER_STATE_OFFSET);
                    if args.len() <= 1 {
                        vars.insert(op.out.unwrap(), res);
                    }
                    builder.ins().jump(next_block, &[]);

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "state_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let pair = vars.get(&args[0]).expect("Yield pair not found");
                    let next_state_id = op.value.unwrap();
                    let self_bits = *vars.get("self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, HEADER_STATE_OFFSET);

                    if has_ret {
                        builder.ins().jump(master_return_block, &[*pair]);
                    } else {
                        builder.ins().jump(master_return_block, &[]);
                    }

                    let next_block = state_blocks[&next_state_id];
                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "chan_send_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = vars.get(&args[0]).expect("Chan not found");
                    let val = vars.get(&args[1]).expect("Val not found");
                    let pending_state_bits = *vars.get(&args[2]).expect("Pending state not found");
                    let next_state_id = op.value.unwrap();
                    let self_bits = *vars.get("self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits);
                    builder.ins().store(
                        MemFlags::new(),
                        pending_state_id,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_send", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan, *val]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(ready_path, current_block);
                    }
                    builder.ins().brif(
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    builder.switch_to_block(ready_path);
                    builder.seal_block(ready_path);
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, HEADER_STATE_OFFSET);
                    vars.insert(op.out.unwrap(), res);
                    builder.ins().jump(next_block, &[]);

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "chan_recv_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = vars.get(&args[0]).expect("Chan not found");
                    let pending_state_bits = *vars.get(&args[1]).expect("Pending state not found");
                    let next_state_id = op.value.unwrap();
                    let self_bits = *vars.get("self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits);
                    builder.ins().store(
                        MemFlags::new(),
                        pending_state_id,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_recv", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(ready_path, current_block);
                    }
                    builder.ins().brif(
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    builder.switch_to_block(ready_path);
                    builder.seal_block(ready_path);
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, HEADER_STATE_OFFSET);
                    vars.insert(op.out.unwrap(), res);
                    builder.ins().jump(next_block, &[]);

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "chan_new" => {
                    let args = op.args.as_ref().unwrap();
                    let capacity = vars.get(&args[0]).expect("Capacity not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*capacity]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "chan_drop" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = vars.get(&args[0]).expect("Chan not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_drop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*chan]);
                }
                "spawn" => {
                    let args = op.args.as_ref().unwrap();
                    let task = vars.get(&args[0]).expect("Task not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_spawn", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*task]);
                }
                "cancel_token_new" => {
                    let args = op.args.as_ref().unwrap();
                    let parent = vars.get(&args[0]).expect("Parent token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*parent]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "cancel_token_clone" => {
                    let args = op.args.as_ref().unwrap();
                    let token = vars.get(&args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_clone", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_drop" => {
                    let args = op.args.as_ref().unwrap();
                    let token = vars.get(&args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_drop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_cancel" => {
                    let args = op.args.as_ref().unwrap();
                    let token = vars.get(&args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_cancel", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_is_cancelled" => {
                    let args = op.args.as_ref().unwrap();
                    let token = vars.get(&args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_is_cancelled", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*token]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "cancel_token_set_current" => {
                    let args = op.args.as_ref().unwrap();
                    let token = vars.get(&args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_set_current", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*token]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "cancel_token_get_current" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_get_current", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "cancelled" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancelled", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "cancel_current" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_current", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[]);
                }
                "call_async" => {
                    let poll_func_name = op.s_value.as_ref().unwrap();
                    if poll_func_name == "molt_async_sleep" {
                        let arg_names = op.args.as_deref().unwrap_or(&[]);
                        let delay_val = arg_names
                            .first()
                            .map(|name| *vars.get(name).expect("Arg not found"))
                            .unwrap_or_else(|| builder.ins().iconst(types::I64, box_float(0.0)));
                        let result_val = arg_names
                            .get(1)
                            .map(|name| *vars.get(name).expect("Arg not found"))
                            .unwrap_or_else(|| builder.ins().iconst(types::I64, box_none()));
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_async_sleep_new", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[delay_val, result_val]);
                        let res = builder.inst_results(call)[0];
                        let out_name = op.out.unwrap();
                        vars.insert(out_name, res);
                    } else {
                        let args = op.args.as_deref();
                        let payload_len = args.map(|vals| vals.len()).unwrap_or(0);
                        let size = builder.ins().iconst(types::I64, (payload_len * 8) as i64);
                        let mut poll_sig = self.module.make_signature();
                        poll_sig.params.push(AbiParam::new(types::I64));
                        poll_sig.returns.push(AbiParam::new(types::I64));
                        let poll_func_id = self
                            .module
                            .declare_function(poll_func_name, Linkage::Import, &poll_sig)
                            .unwrap();
                        let poll_func_ref =
                            self.module.declare_func_in_func(poll_func_id, builder.func);
                        let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let future_callee = self
                            .module
                            .declare_function("molt_future_new", Linkage::Import, &sig)
                            .unwrap();
                        let local_future = self
                            .module
                            .declare_func_in_func(future_callee, builder.func);
                        let call = builder.ins().call(local_future, &[poll_addr, size]);
                        let obj = builder.inst_results(call)[0];
                        let obj_ptr = unbox_ptr_value(&mut builder, obj);

                        if let Some(arg_names) = args {
                            if !arg_names.is_empty() {
                                for (idx, arg_name) in arg_names.iter().enumerate() {
                                    let val = vars.get(arg_name).expect("Arg not found");
                                    builder.ins().store(
                                        MemFlags::new(),
                                        *val,
                                        obj_ptr,
                                        (idx * 8) as i32,
                                    );
                                    builder.ins().call(local_inc_ref_obj, &[*val]);
                                }
                            }
                        }
                        let out_name = op.out.unwrap();
                        vars.insert(out_name, obj);
                    }
                }
                "builtin_func" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap();
                    let mut func_sig = self.module.make_signature();
                    for _ in 0..arity {
                        func_sig.params.push(AbiParam::new(types::I64));
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Import, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let arity_val = builder.ins().iconst(types::I64, arity);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_func_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[func_addr, arity_val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "func_new" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap();
                    let mut func_sig = self.module.make_signature();
                    if func_name.ends_with("_poll") {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let arity_val = builder.ins().iconst(types::I64, arity);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_func_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[func_addr, arity_val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "func_new_closure" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap();
                    let closure_name = op
                        .args
                        .as_ref()
                        .and_then(|args| args.first())
                        .expect("func_new_closure expects closure arg");
                    let closure_bits = *vars.get(closure_name).expect("closure arg not found");
                    let mut func_sig = self.module.make_signature();
                    if func_name.ends_with("_poll") {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        func_sig.params.push(AbiParam::new(types::I64));
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let arity_val = builder.ins().iconst(types::I64, arity);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_func_new_closure", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[func_addr, arity_val, closure_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "code_new" => {
                    let args = op.args.as_ref().unwrap();
                    let filename_bits = vars.get(&args[0]).expect("filename not found");
                    let name_bits = vars.get(&args[1]).expect("name not found");
                    let firstlineno_bits = vars.get(&args[2]).expect("firstlineno not found");
                    let linetable_bits = vars.get(&args[3]).expect("linetable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_code_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            *filename_bits,
                            *name_bits,
                            *firstlineno_bits,
                            *linetable_bits,
                        ],
                    );
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "code_slot_set" => {
                    let args = op.args.as_ref().unwrap();
                    let code_bits = vars.get(&args[0]).expect("code bits not found");
                    let code_id = op.value.unwrap_or(0);
                    let code_id_val = builder.ins().iconst(types::I64, code_id);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_code_slot_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[code_id_val, *code_bits]);
                }
                "code_slots_init" => {
                    let count = op.value.unwrap_or(0);
                    let count_val = builder.ins().iconst(types::I64, count);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_code_slots_init", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[count_val]);
                }
                "line" => {
                    let line = op.value.unwrap_or(0);
                    let line_val = builder.ins().iconst(types::I64, line);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_trace_set_line", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[line_val]);
                }
                "missing" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_missing", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "function_closure_bits" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = vars.get(&args[0]).expect("Func not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_function_closure_bits", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    emit_maybe_ref_adjust(&mut builder, res, local_inc_ref_obj);
                    vars.insert(op.out.unwrap(), res);
                }
                "bound_method_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = vars.get(&args[0]).expect("Func not found");
                    let self_bits = vars.get(&args[1]).expect("Self not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bound_method_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits, *self_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "call" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
                    let mut args = Vec::new();
                    for name in args_names {
                        args.push(*vars.get(name).expect("Arg not found"));
                    }

                    let mut sig = self.module.make_signature();
                    for _ in 0..args.len() {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));

                    let callee = self
                        .module
                        .declare_function(target_name, Linkage::Export, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let mut guard_sig = self.module.make_signature();
                    guard_sig.returns.push(AbiParam::new(types::I64));
                    let guard_enter = self
                        .module
                        .declare_function("molt_recursion_guard_enter", Linkage::Import, &guard_sig)
                        .unwrap();
                    let guard_enter_local =
                        self.module.declare_func_in_func(guard_enter, builder.func);
                    let guard_exit = self
                        .module
                        .declare_function(
                            "molt_recursion_guard_exit",
                            Linkage::Import,
                            &self.module.make_signature(),
                        )
                        .unwrap();
                    let guard_exit_local =
                        self.module.declare_func_in_func(guard_exit, builder.func);
                    let mut trace_sig = self.module.make_signature();
                    trace_sig.params.push(AbiParam::new(types::I64));
                    trace_sig.returns.push(AbiParam::new(types::I64));
                    let trace_enter = self
                        .module
                        .declare_function("molt_trace_enter_slot", Linkage::Import, &trace_sig)
                        .unwrap();
                    let trace_enter_local =
                        self.module.declare_func_in_func(trace_enter, builder.func);
                    let mut trace_exit_sig = self.module.make_signature();
                    trace_exit_sig.returns.push(AbiParam::new(types::I64));
                    let trace_exit = self
                        .module
                        .declare_function("molt_trace_exit", Linkage::Import, &trace_exit_sig)
                        .unwrap();
                    let trace_exit_local =
                        self.module.declare_func_in_func(trace_exit, builder.func);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let call_block = builder.create_block();
                    let fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, call_block, &[], fail_block, &[]);

                    builder.switch_to_block(call_block);
                    builder.seal_block(call_block);
                    let code_id = op.value.unwrap_or(0);
                    let code_id_val = builder.ins().iconst(types::I64, code_id);
                    let _ = builder.ins().call(trace_enter_local, &[code_id_val]);
                    let call = builder.ins().call(local_callee, &args);
                    let res = builder.inst_results(call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[res]);

                    builder.switch_to_block(fail_block);
                    builder.seal_block(fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let res = builder.block_params(merge_block)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "call_guarded" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
                    let callee_bits = vars.get(&args_names[0]).expect("Callee not found");
                    let mut args = Vec::new();
                    for name in &args_names[1..] {
                        args.push(*vars.get(name).expect("Arg not found"));
                    }

                    let mut sig = self.module.make_signature();
                    for _ in 0..args.len() {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));

                    let callee = self
                        .module
                        .declare_function(target_name, Linkage::Export, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let expected_addr = builder.ins().func_addr(types::I64, local_callee);

                    let mut check_sig = self.module.make_signature();
                    check_sig.params.push(AbiParam::new(types::I64));
                    check_sig.returns.push(AbiParam::new(types::I64));
                    let is_func = self
                        .module
                        .declare_function("molt_is_function_obj", Linkage::Import, &check_sig)
                        .unwrap();
                    let is_func_local = self.module.declare_func_in_func(is_func, builder.func);
                    let truthy = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &check_sig)
                        .unwrap();
                    let truthy_local = self.module.declare_func_in_func(truthy, builder.func);
                    let mut guard_sig = self.module.make_signature();
                    guard_sig.returns.push(AbiParam::new(types::I64));
                    let guard_enter = self
                        .module
                        .declare_function("molt_recursion_guard_enter", Linkage::Import, &guard_sig)
                        .unwrap();
                    let guard_enter_local =
                        self.module.declare_func_in_func(guard_enter, builder.func);
                    let guard_exit = self
                        .module
                        .declare_function(
                            "molt_recursion_guard_exit",
                            Linkage::Import,
                            &self.module.make_signature(),
                        )
                        .unwrap();
                    let guard_exit_local =
                        self.module.declare_func_in_func(guard_exit, builder.func);
                    let mut trace_sig = self.module.make_signature();
                    trace_sig.params.push(AbiParam::new(types::I64));
                    trace_sig.returns.push(AbiParam::new(types::I64));
                    let trace_enter = self
                        .module
                        .declare_function("molt_trace_enter", Linkage::Import, &trace_sig)
                        .unwrap();
                    let trace_enter_local =
                        self.module.declare_func_in_func(trace_enter, builder.func);
                    let mut trace_exit_sig = self.module.make_signature();
                    trace_exit_sig.returns.push(AbiParam::new(types::I64));
                    let trace_exit = self
                        .module
                        .declare_function("molt_trace_exit", Linkage::Import, &trace_exit_sig)
                        .unwrap();
                    let trace_exit_local =
                        self.module.declare_func_in_func(trace_exit, builder.func);
                    let is_func_call = builder.ins().call(is_func_local, &[*callee_bits]);
                    let is_func_bits = builder.inst_results(is_func_call)[0];
                    let truthy_call = builder.ins().call(truthy_local, &[is_func_bits]);
                    let truthy_bits = builder.inst_results(truthy_call)[0];
                    let is_func_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);

                    let mut resolve_sig = self.module.make_signature();
                    resolve_sig.params.push(AbiParam::new(types::I64));
                    resolve_sig.returns.push(AbiParam::new(types::I64));
                    let resolve_callee = self
                        .module
                        .declare_function("molt_handle_resolve", Linkage::Import, &resolve_sig)
                        .unwrap();
                    let resolve_local = self
                        .module
                        .declare_func_in_func(resolve_callee, builder.func);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    let func_block = builder.create_block();
                    let fallback_block = builder.create_block();
                    builder
                        .ins()
                        .brif(is_func_bool, func_block, &[], fallback_block, &[]);

                    builder.switch_to_block(fallback_block);
                    builder.seal_block(fallback_block);
                    let mut callargs_sig = self.module.make_signature();
                    callargs_sig.params.push(AbiParam::new(types::I64));
                    callargs_sig.params.push(AbiParam::new(types::I64));
                    callargs_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_new = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &callargs_sig)
                        .unwrap();
                    let callargs_new_local =
                        self.module.declare_func_in_func(callargs_new, builder.func);
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let mut push_sig = self.module.make_signature();
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_push_pos = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &push_sig)
                        .unwrap();
                    let callargs_push_local = self
                        .module
                        .declare_func_in_func(callargs_push_pos, builder.func);
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let mut bind_sig = self.module.make_signature();
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.returns.push(AbiParam::new(types::I64));
                    let call_bind = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &bind_sig)
                        .unwrap();
                    let call_bind_local = self.module.declare_func_in_func(call_bind, builder.func);
                    let fallback_call = builder
                        .ins()
                        .call(call_bind_local, &[*callee_bits, callargs_ptr]);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    builder.ins().jump(merge_block, &[fallback_res]);

                    builder.switch_to_block(func_block);
                    builder.seal_block(func_block);
                    let resolve_call = builder.ins().call(resolve_local, &[*callee_bits]);
                    let func_ptr = builder.inst_results(resolve_call)[0];
                    let fn_ptr = builder.ins().load(types::I64, MemFlags::new(), func_ptr, 0);
                    let matches = builder.ins().icmp(IntCC::Equal, fn_ptr, expected_addr);
                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    builder
                        .ins()
                        .brif(matches, then_block, &[], else_block, &[]);

                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let then_call_block = builder.create_block();
                    let then_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, then_call_block, &[], then_fail_block, &[]);

                    builder.switch_to_block(then_call_block);
                    builder.seal_block(then_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[*callee_bits]);
                    let direct_call = builder.ins().call(local_callee, &args);
                    let direct_res = builder.inst_results(direct_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[direct_res]);

                    builder.switch_to_block(then_fail_block);
                    builder.seal_block(then_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(else_block);
                    builder.seal_block(else_block);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let else_call_block = builder.create_block();
                    let else_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, else_call_block, &[], else_fail_block, &[]);

                    builder.switch_to_block(else_call_block);
                    builder.seal_block(else_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[*callee_bits]);
                    let sig_ref = builder.import_signature(sig);
                    let fallback_call = builder.ins().call_indirect(sig_ref, fn_ptr, &args);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[fallback_res]);

                    builder.switch_to_block(else_fail_block);
                    builder.seal_block(else_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let res = builder.block_params(merge_block)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "call_func" => {
                    let args_names = op.args.as_ref().unwrap();
                    let func_bits = vars.get(&args_names[0]).expect("Func not found");
                    let mut args = Vec::new();
                    for name in &args_names[1..] {
                        args.push(*vars.get(name).expect("Arg not found"));
                    }

                    let mut resolve_sig = self.module.make_signature();
                    resolve_sig.params.push(AbiParam::new(types::I64));
                    resolve_sig.returns.push(AbiParam::new(types::I64));
                    let resolve_callee = self
                        .module
                        .declare_function("molt_handle_resolve", Linkage::Import, &resolve_sig)
                        .unwrap();
                    let resolve_local = self
                        .module
                        .declare_func_in_func(resolve_callee, builder.func);
                    let mut check_sig = self.module.make_signature();
                    check_sig.params.push(AbiParam::new(types::I64));
                    check_sig.returns.push(AbiParam::new(types::I64));
                    let is_bound = self
                        .module
                        .declare_function("molt_is_bound_method", Linkage::Import, &check_sig)
                        .unwrap();
                    let is_bound_local = self.module.declare_func_in_func(is_bound, builder.func);
                    let is_func = self
                        .module
                        .declare_function("molt_is_function_obj", Linkage::Import, &check_sig)
                        .unwrap();
                    let is_func_local = self.module.declare_func_in_func(is_func, builder.func);
                    let truthy = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &check_sig)
                        .unwrap();
                    let truthy_local = self.module.declare_func_in_func(truthy, builder.func);
                    let default_kind = self
                        .module
                        .declare_function("molt_function_default_kind", Linkage::Import, &check_sig)
                        .unwrap();
                    let default_kind_local =
                        self.module.declare_func_in_func(default_kind, builder.func);
                    let closure_bits = self
                        .module
                        .declare_function("molt_function_closure_bits", Linkage::Import, &check_sig)
                        .unwrap();
                    let closure_bits_local =
                        self.module.declare_func_in_func(closure_bits, builder.func);
                    let is_generator = self
                        .module
                        .declare_function("molt_function_is_generator", Linkage::Import, &check_sig)
                        .unwrap();
                    let is_generator_local =
                        self.module.declare_func_in_func(is_generator, builder.func);
                    let is_coroutine = self
                        .module
                        .declare_function("molt_function_is_coroutine", Linkage::Import, &check_sig)
                        .unwrap();
                    let is_coroutine_local =
                        self.module.declare_func_in_func(is_coroutine, builder.func);
                    let mut missing_sig = self.module.make_signature();
                    missing_sig.returns.push(AbiParam::new(types::I64));
                    let missing_fn = self
                        .module
                        .declare_function("molt_missing", Linkage::Import, &missing_sig)
                        .unwrap();
                    let missing_local = self.module.declare_func_in_func(missing_fn, builder.func);
                    let mut guard_sig = self.module.make_signature();
                    guard_sig.returns.push(AbiParam::new(types::I64));
                    let guard_enter = self
                        .module
                        .declare_function("molt_recursion_guard_enter", Linkage::Import, &guard_sig)
                        .unwrap();
                    let guard_enter_local =
                        self.module.declare_func_in_func(guard_enter, builder.func);
                    let guard_exit = self
                        .module
                        .declare_function(
                            "molt_recursion_guard_exit",
                            Linkage::Import,
                            &self.module.make_signature(),
                        )
                        .unwrap();
                    let guard_exit_local =
                        self.module.declare_func_in_func(guard_exit, builder.func);
                    let mut trace_sig = self.module.make_signature();
                    trace_sig.params.push(AbiParam::new(types::I64));
                    trace_sig.returns.push(AbiParam::new(types::I64));
                    let trace_enter = self
                        .module
                        .declare_function("molt_trace_enter", Linkage::Import, &trace_sig)
                        .unwrap();
                    let trace_enter_local =
                        self.module.declare_func_in_func(trace_enter, builder.func);
                    let mut trace_exit_sig = self.module.make_signature();
                    trace_exit_sig.returns.push(AbiParam::new(types::I64));
                    let trace_exit = self
                        .module
                        .declare_function("molt_trace_exit", Linkage::Import, &trace_exit_sig)
                        .unwrap();
                    let trace_exit_local =
                        self.module.declare_func_in_func(trace_exit, builder.func);
                    let is_bound_call = builder.ins().call(is_bound_local, &[*func_bits]);
                    let is_bound_bits = builder.inst_results(is_bound_call)[0];
                    let truthy_call = builder.ins().call(truthy_local, &[is_bound_bits]);
                    let truthy_bits = builder.inst_results(truthy_call)[0];
                    let is_bound_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);

                    let bound_block = builder.create_block();
                    let non_bound_block = builder.create_block();
                    let func_block = builder.create_block();
                    let fallback_block = builder.create_block();
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    builder
                        .ins()
                        .brif(is_bound_bool, bound_block, &[], non_bound_block, &[]);

                    builder.switch_to_block(bound_block);
                    builder.seal_block(bound_block);
                    let method_resolve = builder.ins().call(resolve_local, &[*func_bits]);
                    let method_ptr = builder.inst_results(method_resolve)[0];
                    let bound_func_bits =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::new(), method_ptr, 0);
                    let self_bits = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), method_ptr, 8);
                    let bound_resolve = builder.ins().call(resolve_local, &[bound_func_bits]);
                    let bound_func_ptr = builder.inst_results(bound_resolve)[0];
                    let bound_fn_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::new(), bound_func_ptr, 0);
                    let closure_bits_call =
                        builder.ins().call(closure_bits_local, &[bound_func_bits]);
                    let closure_bits_val = builder.inst_results(closure_bits_call)[0];
                    let closure_is_zero = builder.ins().icmp_imm(IntCC::Equal, closure_bits_val, 0);
                    let is_gen_call = builder.ins().call(is_generator_local, &[bound_func_bits]);
                    let is_gen_bits = builder.inst_results(is_gen_call)[0];
                    let is_gen_truthy_call = builder.ins().call(truthy_local, &[is_gen_bits]);
                    let is_gen_truthy_bits = builder.inst_results(is_gen_truthy_call)[0];
                    let is_gen_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_gen_truthy_bits, 0);
                    let is_coro_call = builder.ins().call(is_coroutine_local, &[bound_func_bits]);
                    let is_coro_bits = builder.inst_results(is_coro_call)[0];
                    let is_coro_truthy_call = builder.ins().call(truthy_local, &[is_coro_bits]);
                    let is_coro_truthy_bits = builder.inst_results(is_coro_truthy_call)[0];
                    let is_coro_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_coro_truthy_bits, 0);
                    let bound_direct_block = builder.create_block();
                    let bound_closure_block = builder.create_block();
                    let bound_non_gen_block = builder.create_block();
                    let bound_non_special_block = builder.create_block();
                    builder.ins().brif(
                        is_gen_bool,
                        bound_closure_block,
                        &[],
                        bound_non_gen_block,
                        &[],
                    );

                    builder.switch_to_block(bound_non_gen_block);
                    builder.seal_block(bound_non_gen_block);
                    builder.ins().brif(
                        is_coro_bool,
                        bound_closure_block,
                        &[],
                        bound_non_special_block,
                        &[],
                    );

                    builder.switch_to_block(bound_non_special_block);
                    builder.seal_block(bound_non_special_block);
                    builder.ins().brif(
                        closure_is_zero,
                        bound_direct_block,
                        &[],
                        bound_closure_block,
                        &[],
                    );

                    builder.switch_to_block(bound_closure_block);
                    builder.seal_block(bound_closure_block);
                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_new = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let callargs_new_local =
                        self.module.declare_func_in_func(callargs_new, builder.func);
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let mut push_sig = self.module.make_signature();
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_push_pos = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &push_sig)
                        .unwrap();
                    let callargs_push_local = self
                        .module
                        .declare_func_in_func(callargs_push_pos, builder.func);
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let mut bind_sig = self.module.make_signature();
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.returns.push(AbiParam::new(types::I64));
                    let call_bind = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &bind_sig)
                        .unwrap();
                    let call_bind_local = self.module.declare_func_in_func(call_bind, builder.func);
                    let bound_call = builder
                        .ins()
                        .call(call_bind_local, &[*func_bits, callargs_ptr]);
                    let bound_res = builder.inst_results(bound_call)[0];
                    builder.ins().jump(merge_block, &[bound_res]);

                    builder.switch_to_block(bound_direct_block);
                    builder.seal_block(bound_direct_block);
                    let bound_arity =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::new(), bound_func_ptr, 8);
                    let provided_arity = builder.ins().iconst(types::I64, (args.len() + 1) as i64);
                    let missing = builder.ins().isub(bound_arity, provided_arity);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let one = builder.ins().iconst(types::I64, 1);
                    let two = builder.ins().iconst(types::I64, 2);
                    let is_zero = builder.ins().icmp(IntCC::Equal, missing, zero);
                    let is_one = builder.ins().icmp(IntCC::Equal, missing, one);
                    let is_two = builder.ins().icmp(IntCC::Equal, missing, two);
                    let default_kind_call =
                        builder.ins().call(default_kind_local, &[bound_func_bits]);
                    let default_kind_val = builder.inst_results(default_kind_call)[0];
                    let default_none = builder.ins().iconst(types::I64, FUNC_DEFAULT_NONE);
                    let default_pop = builder.ins().iconst(types::I64, FUNC_DEFAULT_DICT_POP);
                    let default_update = builder.ins().iconst(types::I64, FUNC_DEFAULT_DICT_UPDATE);

                    let bound_exact_block = builder.create_block();
                    let bound_missing_one_block = builder.create_block();
                    let bound_missing_two_block = builder.create_block();
                    let bound_error_block = builder.create_block();
                    let bound_missing_check = builder.create_block();
                    let bound_missing_two_check = builder.create_block();

                    builder
                        .ins()
                        .brif(is_zero, bound_exact_block, &[], bound_missing_check, &[]);

                    builder.switch_to_block(bound_missing_check);
                    builder.seal_block(bound_missing_check);
                    builder.ins().brif(
                        is_one,
                        bound_missing_one_block,
                        &[],
                        bound_missing_two_check,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_two_check);
                    builder.seal_block(bound_missing_two_check);
                    builder.ins().brif(
                        is_two,
                        bound_missing_two_block,
                        &[],
                        bound_error_block,
                        &[],
                    );

                    builder.switch_to_block(bound_exact_block);
                    builder.seal_block(bound_exact_block);
                    let mut bound_args = Vec::with_capacity(args.len() + 1);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_one_block);
                    builder.seal_block(bound_missing_one_block);
                    let is_default_none =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_none);
                    let is_default_pop =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_pop);
                    let is_default_update =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_update);
                    let bound_missing_one_default = builder.create_block();
                    let bound_missing_one_pop = builder.create_block();
                    let bound_missing_one_update = builder.create_block();
                    let bound_missing_one_check = builder.create_block();
                    builder.ins().brif(
                        is_default_none,
                        bound_missing_one_default,
                        &[],
                        bound_missing_one_check,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_one_check);
                    builder.seal_block(bound_missing_one_check);
                    builder.ins().brif(
                        is_default_pop,
                        bound_missing_one_pop,
                        &[],
                        bound_missing_one_update,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_one_default);
                    builder.seal_block(bound_missing_one_default);
                    let mut bound_args = Vec::with_capacity(args.len() + 2);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    bound_args.push(none_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_one_pop);
                    builder.seal_block(bound_missing_one_pop);
                    let mut bound_args = Vec::with_capacity(args.len() + 2);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let has_default_bits = builder.ins().iconst(types::I64, box_int(1));
                    bound_args.push(has_default_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_one_update);
                    builder.seal_block(bound_missing_one_update);
                    let bound_missing_one_update_ok = builder.create_block();
                    builder.ins().brif(
                        is_default_update,
                        bound_missing_one_update_ok,
                        &[],
                        bound_error_block,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_one_update_ok);
                    builder.seal_block(bound_missing_one_update_ok);
                    let missing_call = builder.ins().call(missing_local, &[]);
                    let missing_bits = builder.inst_results(missing_call)[0];
                    let mut bound_args = Vec::with_capacity(args.len() + 2);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    bound_args.push(missing_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_two_block);
                    builder.seal_block(bound_missing_two_block);
                    let is_default_pop =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_pop);
                    let bound_missing_two_ok = builder.create_block();
                    builder.ins().brif(
                        is_default_pop,
                        bound_missing_two_ok,
                        &[],
                        bound_error_block,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_two_ok);
                    builder.seal_block(bound_missing_two_ok);
                    let mut bound_args = Vec::with_capacity(args.len() + 3);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    let has_default_bits = builder.ins().iconst(types::I64, box_int(0));
                    bound_args.push(none_bits);
                    bound_args.push(has_default_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(bound_error_block);
                    builder.seal_block(bound_error_block);
                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_new = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let callargs_new_local =
                        self.module.declare_func_in_func(callargs_new, builder.func);
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let mut push_sig = self.module.make_signature();
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_push_pos = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &push_sig)
                        .unwrap();
                    let callargs_push_local = self
                        .module
                        .declare_func_in_func(callargs_push_pos, builder.func);
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let mut bind_sig = self.module.make_signature();
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.returns.push(AbiParam::new(types::I64));
                    let call_bind = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &bind_sig)
                        .unwrap();
                    let call_bind_local = self.module.declare_func_in_func(call_bind, builder.func);
                    let fallback_call = builder
                        .ins()
                        .call(call_bind_local, &[*func_bits, callargs_ptr]);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    builder.ins().jump(merge_block, &[fallback_res]);

                    builder.switch_to_block(non_bound_block);
                    builder.seal_block(non_bound_block);
                    let is_func_call = builder.ins().call(is_func_local, &[*func_bits]);
                    let is_func_bits = builder.inst_results(is_func_call)[0];
                    let truthy_call = builder.ins().call(truthy_local, &[is_func_bits]);
                    let truthy_bits = builder.inst_results(truthy_call)[0];
                    let is_func_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);
                    builder
                        .ins()
                        .brif(is_func_bool, func_block, &[], fallback_block, &[]);

                    builder.switch_to_block(fallback_block);
                    builder.seal_block(fallback_block);
                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_new = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let callargs_new_local =
                        self.module.declare_func_in_func(callargs_new, builder.func);
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let mut push_sig = self.module.make_signature();
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_push_pos = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &push_sig)
                        .unwrap();
                    let callargs_push_local = self
                        .module
                        .declare_func_in_func(callargs_push_pos, builder.func);
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let mut bind_sig = self.module.make_signature();
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.returns.push(AbiParam::new(types::I64));
                    let call_bind = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &bind_sig)
                        .unwrap();
                    let call_bind_local = self.module.declare_func_in_func(call_bind, builder.func);
                    let fallback_call = builder
                        .ins()
                        .call(call_bind_local, &[*func_bits, callargs_ptr]);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    builder.ins().jump(merge_block, &[fallback_res]);

                    builder.switch_to_block(func_block);
                    builder.seal_block(func_block);
                    let closure_bits_call = builder.ins().call(closure_bits_local, &[*func_bits]);
                    let closure_bits_val = builder.inst_results(closure_bits_call)[0];
                    let closure_is_zero = builder.ins().icmp_imm(IntCC::Equal, closure_bits_val, 0);
                    let is_gen_call = builder.ins().call(is_generator_local, &[*func_bits]);
                    let is_gen_bits = builder.inst_results(is_gen_call)[0];
                    let is_gen_truthy_call = builder.ins().call(truthy_local, &[is_gen_bits]);
                    let is_gen_truthy_bits = builder.inst_results(is_gen_truthy_call)[0];
                    let is_gen_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_gen_truthy_bits, 0);
                    let is_coro_call = builder.ins().call(is_coroutine_local, &[*func_bits]);
                    let is_coro_bits = builder.inst_results(is_coro_call)[0];
                    let is_coro_truthy_call = builder.ins().call(truthy_local, &[is_coro_bits]);
                    let is_coro_truthy_bits = builder.inst_results(is_coro_truthy_call)[0];
                    let is_coro_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_coro_truthy_bits, 0);
                    let func_direct_block = builder.create_block();
                    let func_closure_block = builder.create_block();
                    let func_non_gen_block = builder.create_block();
                    let func_non_special_block = builder.create_block();
                    builder.ins().brif(
                        is_gen_bool,
                        func_closure_block,
                        &[],
                        func_non_gen_block,
                        &[],
                    );

                    builder.switch_to_block(func_non_gen_block);
                    builder.seal_block(func_non_gen_block);
                    builder.ins().brif(
                        is_coro_bool,
                        func_closure_block,
                        &[],
                        func_non_special_block,
                        &[],
                    );

                    builder.switch_to_block(func_non_special_block);
                    builder.seal_block(func_non_special_block);
                    builder.ins().brif(
                        closure_is_zero,
                        func_direct_block,
                        &[],
                        func_closure_block,
                        &[],
                    );

                    builder.switch_to_block(func_closure_block);
                    builder.seal_block(func_closure_block);
                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_new = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let callargs_new_local =
                        self.module.declare_func_in_func(callargs_new, builder.func);
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let mut push_sig = self.module.make_signature();
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_push_pos = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &push_sig)
                        .unwrap();
                    let callargs_push_local = self
                        .module
                        .declare_func_in_func(callargs_push_pos, builder.func);
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let mut bind_sig = self.module.make_signature();
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.returns.push(AbiParam::new(types::I64));
                    let call_bind = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &bind_sig)
                        .unwrap();
                    let call_bind_local = self.module.declare_func_in_func(call_bind, builder.func);
                    let closure_call = builder
                        .ins()
                        .call(call_bind_local, &[*func_bits, callargs_ptr]);
                    let closure_res = builder.inst_results(closure_call)[0];
                    builder.ins().jump(merge_block, &[closure_res]);

                    builder.switch_to_block(func_direct_block);
                    builder.seal_block(func_direct_block);
                    let resolve_call = builder.ins().call(resolve_local, &[*func_bits]);
                    let func_ptr = builder.inst_results(resolve_call)[0];
                    let fn_ptr = builder.ins().load(types::I64, MemFlags::new(), func_ptr, 0);

                    let mut sig = self.module.make_signature();
                    for _ in 0..args.len() {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let func_call_block = builder.create_block();
                    let func_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, func_call_block, &[], func_fail_block, &[]);

                    builder.switch_to_block(func_call_block);
                    builder.seal_block(func_call_block);
                    let _ = builder.ins().call(trace_enter_local, &[*func_bits]);
                    let call = builder.ins().call_indirect(sig_ref, fn_ptr, &args);
                    let res = builder.inst_results(call)[0];
                    let _ = builder.ins().call(trace_exit_local, &[]);
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    builder.ins().jump(merge_block, &[res]);

                    builder.switch_to_block(func_fail_block);
                    builder.seal_block(func_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    builder.ins().jump(merge_block, &[none_bits]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let res = builder.block_params(merge_block)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "call_bind" => {
                    let args_names = op.args.as_ref().unwrap();
                    let func_bits = vars.get(&args_names[0]).expect("Func not found");
                    let builder_ptr = vars.get(&args_names[1]).expect("Callargs not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*func_bits, *builder_ptr]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "call_method" => {
                    let args_names = op.args.as_ref().unwrap();
                    let method_bits = vars.get(&args_names[0]).expect("Method not found");
                    let mut extra_args = Vec::new();
                    for name in &args_names[1..] {
                        extra_args.push(*vars.get(name).expect("Arg not found"));
                    }
                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_new = self
                        .module
                        .declare_function("molt_callargs_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let callargs_new_local =
                        self.module.declare_func_in_func(callargs_new, builder.func);
                    let pos_capacity = builder.ins().iconst(types::I64, extra_args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let mut push_sig = self.module.make_signature();
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.params.push(AbiParam::new(types::I64));
                    push_sig.returns.push(AbiParam::new(types::I64));
                    let callargs_push_pos = self
                        .module
                        .declare_function("molt_callargs_push_pos", Linkage::Import, &push_sig)
                        .unwrap();
                    let callargs_push_local = self
                        .module
                        .declare_func_in_func(callargs_push_pos, builder.func);
                    for arg in &extra_args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let mut bind_sig = self.module.make_signature();
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.params.push(AbiParam::new(types::I64));
                    bind_sig.returns.push(AbiParam::new(types::I64));
                    let call_bind = self
                        .module
                        .declare_function("molt_call_bind", Linkage::Import, &bind_sig)
                        .unwrap();
                    let call_bind_local = self.module.declare_func_in_func(call_bind, builder.func);
                    let call = builder
                        .ins()
                        .call(call_bind_local, &[*method_bits, callargs_ptr]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "module_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits = vars.get(&args[0]).expect("Module name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "class_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits = vars.get(&args[0]).expect("Class name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "builtin_type" => {
                    let args = op.args.as_ref().unwrap();
                    let tag_bits = vars.get(&args[0]).expect("Tag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_builtin_type", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tag_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "type_of" => {
                    let args = op.args.as_ref().unwrap();
                    let obj_bits = vars.get(&args[0]).expect("Object not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_type_of", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "class_layout_version" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_layout_version", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "class_set_layout_version" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let version_bits = vars.get(&args[1]).expect("Version not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_set_layout_version", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*class_bits, *version_bits]);
                    if let Some(out_name) = op.out.as_ref() {
                        if out_name != "none" {
                            let res = builder.inst_results(call)[0];
                            vars.insert(out_name.clone(), res);
                        }
                    }
                }
                "isinstance" => {
                    let args = op.args.as_ref().unwrap();
                    let obj_bits = vars.get(&args[0]).expect("Object not found");
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_isinstance", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "issubclass" => {
                    let args = op.args.as_ref().unwrap();
                    let sub_bits = vars.get(&args[0]).expect("Subclass not found");
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_issubclass", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*sub_bits, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "object_new" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_object_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "class_set_base" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let base_bits = vars.get(&args[1]).expect("Base class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_set_base", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits, *base_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "class_apply_set_name" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_apply_set_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "super_new" => {
                    let args = op.args.as_ref().unwrap();
                    let type_bits = vars.get(&args[0]).expect("Type not found");
                    let obj_bits = vars.get(&args[1]).expect("Object not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_super_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*type_bits, *obj_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "classmethod_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = vars.get(&args[0]).expect("Func not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_classmethod_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "staticmethod_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = vars.get(&args[0]).expect("Func not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_staticmethod_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "property_new" => {
                    let args = op.args.as_ref().unwrap();
                    let getter = vars.get(&args[0]).expect("Getter not found");
                    let setter = vars.get(&args[1]).expect("Setter not found");
                    let deleter = vars.get(&args[2]).expect("Deleter not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_property_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*getter, *setter, *deleter]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "object_set_class" => {
                    let args = op.args.as_ref().unwrap();
                    let obj_bits = vars.get(&args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj_bits);
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_object_set_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "module_cache_get" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits = vars.get(&args[0]).expect("Module name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_cache_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "module_cache_set" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits = vars.get(&args[0]).expect("Module name not found");
                    let module_bits = vars.get(&args[1]).expect("Module not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_cache_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*name_bits, *module_bits]);
                }
                "module_get_attr" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits = vars.get(&args[0]).expect("Module not found");
                    let attr_bits = vars.get(&args[1]).expect("Attr not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_get_attr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "module_get_global" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits = vars.get(&args[0]).expect("Module not found");
                    let attr_bits = vars.get(&args[1]).expect("Attr not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_get_global", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "module_get_name" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits = vars.get(&args[0]).expect("Module not found");
                    let attr_bits = vars.get(&args[1]).expect("Attr not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_get_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "module_set_attr" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits = vars.get(&args[0]).expect("Module not found");
                    let attr_bits = vars.get(&args[1]).expect("Attr not found");
                    let val_bits = vars.get(&args[2]).unwrap_or_else(|| {
                        panic!(
                            "Value not found for module_set_attr in {} op {}",
                            func_ir.name, op_idx
                        )
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_set_attr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits, *val_bits]);
                }
                "context_null" => {
                    let args = op.args.as_ref().unwrap();
                    let payload = vars.get(&args[0]).expect("Payload not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_null", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*payload]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "context_enter" => {
                    let args = op.args.as_ref().unwrap();
                    let ctx = vars.get(&args[0]).expect("Context not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_enter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*ctx]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "context_exit" => {
                    let args = op.args.as_ref().unwrap();
                    let ctx = vars.get(&args[0]).expect("Context not found");
                    let exc = vars.get(&args[1]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_exit", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*ctx, *exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "context_closing" => {
                    let args = op.args.as_ref().unwrap();
                    let payload = vars.get(&args[0]).expect("Payload not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_closing", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*payload]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "context_unwind" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_unwind", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "context_depth" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_depth", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "context_unwind_to" => {
                    let args = op.args.as_ref().unwrap();
                    let depth = vars.get(&args[0]).expect("Depth not found");
                    let exc = vars.get(&args[1]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_unwind_to", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*depth, *exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_push" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_push", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_pop" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_last" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_last", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_new" => {
                    let args = op.args.as_ref().unwrap();
                    let kind = vars.get(&args[0]).expect("Kind not found");
                    let msg = vars.get(&args[1]).expect("Message not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*kind, *msg]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_clear" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_kind" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_kind", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_message" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_message", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_set_cause" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let cause = vars.get(&args[1]).expect("Cause not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_set_cause", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *cause]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_set_value" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let value = vars.get(&args[1]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_set_value", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *value]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "exception_context_set" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_context_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "raise" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = vars.get(&args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_raise", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "check_exception" => {
                    let target_id = op.value.unwrap();
                    let target_block = state_blocks[&target_id];
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_pending", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let pending = builder.inst_results(call)[0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, pending, 0);
                    let fallthrough = builder.create_block();
                    builder
                        .ins()
                        .brif(cond, target_block, &[], fallthrough, &[]);
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(fallthrough, current_block);
                    }
                    builder.switch_to_block(fallthrough);
                    builder.seal_block(fallthrough);
                }
                "file_open" => {
                    let args = op.args.as_ref().unwrap();
                    let path = vars.get(&args[0]).expect("Path not found");
                    let mode = vars.get(&args[1]).expect("Mode not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_open", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*path, *mode]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "file_read" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = vars.get(&args[0]).expect("Handle not found");
                    let size = vars.get(&args[1]).expect("Size not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_read", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle, *size]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "file_write" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = vars.get(&args[0]).expect("Handle not found");
                    let data = vars.get(&args[1]).expect("Data not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_write", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle, *data]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "file_close" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = vars.get(&args[0]).expect("Handle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_close", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bridge_unavailable" => {
                    let args = op.args.as_ref().unwrap();
                    let msg = vars.get(&args[0]).expect("Message not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bridge_unavailable", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*msg]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "if" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = vars.get(&args[0]).expect("Cond not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    let merge_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(then_block, current_block);
                        builder.insert_block_after(else_block, then_block);
                        builder.insert_block_after(merge_block, else_block);
                    }
                    builder
                        .ins()
                        .brif(cond_bool, then_block, &[], else_block, &[]);
                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    is_block_filled = false;
                    if_stack.push(IfFrame {
                        else_block,
                        merge_block,
                        has_else: false,
                        then_exit: None,
                        else_exit: None,
                        then_terminal: false,
                        else_terminal: false,
                        phi_ops: None,
                        vars_snapshot: vars.clone(),
                        vars_then: None,
                    });
                }
                "else" => {
                    let frame = if_stack.last_mut().expect("No if on stack");
                    if let Some(block) = builder.current_block() {
                        frame.then_exit = Some((block, is_block_filled));
                    }
                    frame.then_terminal = is_block_filled;
                    let then_vars = vars.clone();
                    frame.vars_then = Some(then_vars.clone());
                    if !is_block_filled {
                        if frame.phi_ops.is_none() {
                            let mut depth = 0usize;
                            let mut scan = op_idx + 1;
                            let mut end_if_idx = None;
                            while scan < ops.len() {
                                match ops[scan].kind.as_str() {
                                    "if" => depth += 1,
                                    "end_if" => {
                                        if depth == 0 {
                                            end_if_idx = Some(scan);
                                            break;
                                        }
                                        depth -= 1;
                                    }
                                    _ => {}
                                }
                                scan += 1;
                            }
                            let end_if_idx = end_if_idx.expect("else without matching end_if");
                            let mut phi_ops: Vec<(String, String, String)> = Vec::new();
                            let mut scan_idx = end_if_idx + 1;
                            while scan_idx < ops.len() {
                                let next = &ops[scan_idx];
                                if next.kind != "phi" {
                                    break;
                                }
                                let args = next.args.as_ref().expect("phi args missing");
                                if args.len() != 2 {
                                    panic!("phi expects exactly two args");
                                }
                                let out = next.out.clone().expect("phi output missing");
                                phi_ops.push((out, args[0].clone(), args[1].clone()));
                                skip_ops.insert(scan_idx);
                                scan_idx += 1;
                            }
                            if !phi_ops.is_empty() {
                                for _ in 0..phi_ops.len() {
                                    builder.append_block_param(frame.merge_block, types::I64);
                                }
                                for (idx, (out, _, _)) in phi_ops.iter().enumerate() {
                                    let param = builder.block_params(frame.merge_block)[idx];
                                    vars.insert(out.clone(), param);
                                }
                            }
                            frame.phi_ops = Some(phi_ops);
                        }
                        let phi_ops = frame.phi_ops.clone().unwrap_or_default();
                        let mut then_args: Vec<Value> = Vec::new();
                        for (_, then_name, _) in &phi_ops {
                            let then_val = *then_vars
                                .get(then_name)
                                .unwrap_or_else(|| panic!("phi arg not found: {then_name}"));
                            then_args.push(then_val);
                        }
                        if let Some(block) = builder.current_block() {
                            if let Some(names) = block_tracked_obj.get_mut(&block) {
                                let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                                for tracked in cleanup {
                                    builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                                }
                            }
                            if let Some(names) = block_tracked_ptr.get_mut(&block) {
                                let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                                for tracked in cleanup {
                                    builder.ins().call(local_dec_ref, &[tracked.value]);
                                }
                            }
                            builder.ins().jump(frame.merge_block, &then_args);
                            frame.then_exit = Some((block, true));
                        }
                    }
                    vars = frame.vars_snapshot.clone();
                    if let Some(phi_ops) = frame.phi_ops.as_ref() {
                        if !phi_ops.is_empty() {
                            for (idx, (out, _, _)) in phi_ops.iter().enumerate() {
                                let param = builder.block_params(frame.merge_block)[idx];
                                vars.insert(out.clone(), param);
                            }
                        }
                    }
                    builder.switch_to_block(frame.else_block);
                    builder.seal_block(frame.else_block);
                    is_block_filled = false;
                    frame.has_else = true;
                }
                "end_if" => {
                    let mut frame = if_stack.pop().expect("No if on stack");
                    let current_block = builder
                        .current_block()
                        .expect("end_if requires an active block");

                    if frame.has_else {
                        frame.else_exit = Some((current_block, is_block_filled));
                        frame.else_terminal = is_block_filled;
                    } else {
                        frame.then_exit = Some((current_block, is_block_filled));
                        frame.else_exit = Some((frame.else_block, false));
                        frame.then_terminal = is_block_filled;
                        frame.else_terminal = false;
                    }

                    let then_vars = frame.vars_then.take().unwrap_or_else(|| vars.clone());
                    let else_vars = if frame.has_else {
                        vars.clone()
                    } else {
                        frame.vars_snapshot.clone()
                    };
                    let phi_prepared = frame.phi_ops.is_some();
                    let phi_ops: Vec<(String, String, String)> =
                        if let Some(phi_ops) = frame.phi_ops.take() {
                            phi_ops
                        } else {
                            let mut phi_ops: Vec<(String, String, String)> = Vec::new();
                            let mut scan_idx = op_idx + 1;
                            while scan_idx < ops.len() {
                                let next = &ops[scan_idx];
                                if next.kind != "phi" {
                                    break;
                                }
                                let args = next.args.as_ref().expect("phi args missing");
                                if args.len() != 2 {
                                    panic!("phi expects exactly two args");
                                }
                                let out = next.out.clone().expect("phi output missing");
                                phi_ops.push((out, args[0].clone(), args[1].clone()));
                                skip_ops.insert(scan_idx);
                                scan_idx += 1;
                            }
                            phi_ops
                        };

                    if !phi_prepared && !phi_ops.is_empty() {
                        for _ in 0..phi_ops.len() {
                            builder.append_block_param(frame.merge_block, types::I64);
                        }
                        for (idx, (out, _, _)) in phi_ops.iter().enumerate() {
                            let param = builder.block_params(frame.merge_block)[idx];
                            vars.insert(out.clone(), param);
                        }
                    }

                    let mut then_args: Vec<Value> = Vec::new();
                    let mut else_args: Vec<Value> = Vec::new();
                    for (_, then_name, else_name) in &phi_ops {
                        let then_val = *then_vars
                            .get(then_name)
                            .unwrap_or_else(|| panic!("phi arg not found: {then_name}"));
                        let else_val = *else_vars
                            .get(else_name)
                            .unwrap_or_else(|| panic!("phi arg not found: {else_name}"));
                        then_args.push(then_val);
                        else_args.push(else_val);
                    }

                    let both_filled = frame.then_terminal && frame.else_terminal;

                    let current_block = builder.current_block();
                    let mut emit_jump = |block: Block, filled: bool, args: &[Value]| {
                        if filled {
                            return;
                        }
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref, &[tracked.value]);
                            }
                        }
                        if builder.current_block() != Some(block) {
                            builder.switch_to_block(block);
                        }
                        builder.ins().jump(frame.merge_block, args);
                    };

                    if let Some(current_block) = current_block {
                        if let Some((block, filled)) = frame.then_exit {
                            if block == current_block {
                                emit_jump(block, filled, &then_args);
                                frame.then_exit = None;
                            }
                        }
                        if let Some((block, filled)) = frame.else_exit {
                            if block == current_block {
                                emit_jump(block, filled, &else_args);
                                frame.else_exit = None;
                            }
                        }
                    }

                    if let Some((block, filled)) = frame.then_exit {
                        emit_jump(block, filled, &then_args);
                    }
                    if let Some((block, filled)) = frame.else_exit {
                        emit_jump(block, filled, &else_args);
                        if !frame.has_else && block == frame.else_block {
                            builder.seal_block(block);
                        }
                    }

                    builder.switch_to_block(frame.merge_block);
                    if both_filled {
                        builder.ins().trap(TrapCode::unwrap_user(1));
                        builder.seal_block(frame.merge_block);
                        is_block_filled = true;
                    } else {
                        builder.seal_block(frame.merge_block);
                        is_block_filled = false;
                    }
                    vars = frame.vars_snapshot;
                    if !phi_ops.is_empty() {
                        for (idx, (out, _, _)) in phi_ops.iter().enumerate() {
                            let param = builder.block_params(frame.merge_block)[idx];
                            vars.insert(out.clone(), param);
                        }
                    }
                }
                "loop_start" => {
                    let current_block = builder
                        .current_block()
                        .expect("loop_start requires an active block");
                    let loop_block = builder.create_block();
                    let body_block = builder.create_block();
                    let after_block = builder.create_block();
                    builder.insert_block_after(loop_block, current_block);
                    builder.insert_block_after(body_block, loop_block);
                    builder.insert_block_after(after_block, body_block);
                    if !is_block_filled {
                        builder.ins().jump(loop_block, &[]);
                    }
                    builder.switch_to_block(loop_block);
                    loop_stack.push(LoopFrame {
                        loop_block,
                        body_block,
                        after_block,
                        index_name: None,
                        next_index: None,
                    });
                    loop_depth += 1;
                    is_block_filled = false;
                }
                "loop_index_start" => {
                    let args = op.args.as_ref().unwrap();
                    let start = vars.get(&args[0]).expect("Loop index start not found");
                    let current_block = builder
                        .current_block()
                        .expect("loop_index_start requires an active block");
                    let loop_block = builder.create_block();
                    let body_block = builder.create_block();
                    let after_block = builder.create_block();
                    let idx_param = builder.append_block_param(loop_block, types::I64);
                    builder.insert_block_after(loop_block, current_block);
                    builder.insert_block_after(body_block, loop_block);
                    builder.insert_block_after(after_block, body_block);
                    if !is_block_filled {
                        builder.ins().jump(loop_block, &[*start]);
                    }
                    builder.switch_to_block(loop_block);
                    let out_name = op.out.unwrap();
                    vars.insert(out_name.clone(), idx_param);
                    loop_stack.push(LoopFrame {
                        loop_block,
                        body_block,
                        after_block,
                        index_name: Some(out_name),
                        next_index: None,
                    });
                    loop_depth += 1;
                    is_block_filled = false;
                }
                "loop_break_if_true" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = vars.get(&args[0]).expect("Loop break cond not found");
                    let frame = loop_stack.last().expect("No loop on stack");
                    let current_block = builder
                        .current_block()
                        .expect("loop_break_if_true requires an active block");
                    let tracked_obj_snapshot = block_tracked_obj
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let tracked_ptr_snapshot = block_tracked_ptr
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                    let cleanup_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(cleanup_block, current_block);
                    }
                    builder
                        .ins()
                        .brif(cond_bool, cleanup_block, &[], frame.body_block, &[]);
                    builder.switch_to_block(cleanup_block);
                    builder.seal_block(cleanup_block);
                    for tracked in tracked_obj_snapshot {
                        builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                    }
                    for tracked in tracked_ptr_snapshot {
                        builder.ins().call(local_dec_ref, &[tracked.value]);
                    }
                    builder.ins().jump(frame.after_block, &[]);
                    builder.switch_to_block(frame.body_block);
                    builder.seal_block(frame.body_block);
                    is_block_filled = false;
                }
                "loop_break_if_false" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = vars.get(&args[0]).expect("Loop break cond not found");
                    let frame = loop_stack.last().expect("No loop on stack");
                    let current_block = builder
                        .current_block()
                        .expect("loop_break_if_false requires an active block");
                    let tracked_obj_snapshot = block_tracked_obj
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let tracked_ptr_snapshot = block_tracked_ptr
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                    let cleanup_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(cleanup_block, current_block);
                    }
                    builder
                        .ins()
                        .brif(cond_bool, frame.body_block, &[], cleanup_block, &[]);
                    builder.switch_to_block(cleanup_block);
                    builder.seal_block(cleanup_block);
                    for tracked in tracked_obj_snapshot {
                        builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                    }
                    for tracked in tracked_ptr_snapshot {
                        builder.ins().call(local_dec_ref, &[tracked.value]);
                    }
                    builder.ins().jump(frame.after_block, &[]);
                    builder.switch_to_block(frame.body_block);
                    builder.seal_block(frame.body_block);
                    is_block_filled = false;
                }
                "loop_break" => {
                    let frame = loop_stack.last().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    let current_block = builder
                        .current_block()
                        .expect("loop_break requires an active block");
                    if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for tracked in cleanup {
                            builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for tracked in cleanup {
                            builder.ins().call(local_dec_ref, &[tracked.value]);
                        }
                    }
                    builder.ins().jump(frame.after_block, &[]);
                    is_block_filled = true;
                }
                "loop_index_next" => {
                    let args = op.args.as_ref().unwrap();
                    let next_idx = vars.get(&args[0]).expect("Loop index next not found");
                    let frame = loop_stack.last_mut().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    frame.next_index = Some(*next_idx);
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, *next_idx);
                }
                "loop_continue" => {
                    let frame = loop_stack.last_mut().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    let current_block = builder
                        .current_block()
                        .expect("loop_continue requires an active block");
                    if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for tracked in cleanup {
                            builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for tracked in cleanup {
                            builder.ins().call(local_dec_ref, &[tracked.value]);
                        }
                    }
                    if let Some(next_idx) = frame.next_index.take() {
                        builder.ins().jump(frame.loop_block, &[next_idx]);
                    } else if let Some(name) = frame.index_name.as_ref() {
                        let current_idx = vars.get(name).expect("Loop index not found");
                        builder.ins().jump(frame.loop_block, &[*current_idx]);
                    } else {
                        builder.ins().jump(frame.loop_block, &[]);
                    }
                    is_block_filled = true;
                }
                "loop_end" => {
                    let frame = loop_stack.pop().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    loop_depth -= 1;
                    if !is_block_filled {
                        if let Some(name) = frame.index_name.as_ref() {
                            let current_idx = vars.get(name).expect("Loop index not found");
                            builder.ins().jump(frame.loop_block, &[*current_idx]);
                        } else {
                            builder.ins().jump(frame.loop_block, &[]);
                        }
                    }
                    builder.seal_block(frame.loop_block);
                    builder.switch_to_block(frame.after_block);
                    builder.seal_block(frame.after_block);
                    is_block_filled = false;
                }
                "alloc" => {
                    let size = op.value.unwrap();
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "alloc_class" => {
                    let size = op.value.unwrap();
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "alloc_class_trusted" => {
                    let size = op.value.unwrap();
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc_class_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "alloc_class_static" => {
                    let size = op.value.unwrap();
                    let args = op.args.as_ref().unwrap();
                    let class_bits = vars.get(&args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc_class_static", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "alloc_future" => {
                    let closure_size = op.value.unwrap();
                    let size = builder.ins().iconst(types::I64, closure_size);

                    let poll_func_name = op.s_value.as_ref().unwrap();
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));

                    let poll_func_id = self
                        .module
                        .declare_function(poll_func_name, Linkage::Export, &poll_sig)
                        .unwrap();
                    let poll_func_ref =
                        self.module.declare_func_in_func(poll_func_id, builder.func);
                    let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let future_callee = self
                        .module
                        .declare_function("molt_future_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_future = self
                        .module
                        .declare_func_in_func(future_callee, builder.func);
                    let call = builder.ins().call(local_future, &[poll_addr, size]);
                    let obj = builder.inst_results(call)[0];
                    let obj_ptr = unbox_ptr_value(&mut builder, obj);

                    if let Some(args_names) = &op.args {
                        for (i, name) in args_names.iter().enumerate() {
                            let arg_val = vars.get(name).expect("Arg not found for alloc_future");
                            let offset = (i * 8) as i32;
                            builder
                                .ins()
                                .store(MemFlags::new(), *arg_val, obj_ptr, offset);
                            emit_maybe_ref_adjust(&mut builder, *arg_val, local_inc_ref_obj);
                        }
                    }

                    let out_name = op.out.unwrap();
                    vars.insert(out_name, obj);
                }
                "alloc_generator" => {
                    let closure_size = op.value.unwrap();
                    let size = builder.ins().iconst(types::I64, closure_size);

                    let poll_func_name = op.s_value.as_ref().unwrap();
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));

                    let poll_func_id = self
                        .module
                        .declare_function(poll_func_name, Linkage::Export, &poll_sig)
                        .unwrap();
                    let poll_func_ref =
                        self.module.declare_func_in_func(poll_func_id, builder.func);
                    let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                    let mut gen_sig = self.module.make_signature();
                    gen_sig.params.push(AbiParam::new(types::I64));
                    gen_sig.params.push(AbiParam::new(types::I64));
                    gen_sig.returns.push(AbiParam::new(types::I64));
                    let gen_callee = self
                        .module
                        .declare_function("molt_generator_new", Linkage::Import, &gen_sig)
                        .unwrap();
                    let gen_local = self.module.declare_func_in_func(gen_callee, builder.func);
                    let call = builder.ins().call(gen_local, &[poll_addr, size]);
                    let obj = builder.inst_results(call)[0];
                    let obj_ptr = unbox_ptr_value(&mut builder, obj);

                    if let Some(args_names) = &op.args {
                        for (i, name) in args_names.iter().enumerate() {
                            let arg_val =
                                vars.get(name).expect("Arg not found for alloc_generator");
                            let offset = GENERATOR_CONTROL_BYTES + (i * 8) as i32;
                            builder
                                .ins()
                                .store(MemFlags::new(), *arg_val, obj_ptr, offset);
                            emit_maybe_ref_adjust(&mut builder, *arg_val, local_inc_ref_obj);
                        }
                    }

                    output_is_ptr = false;
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, obj);
                }
                "store" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let val = vars.get(&args[1]).expect("Value not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let profile_block = builder.create_block();
                    let profile_cont = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(profile_block, current_block);
                        builder.insert_block_after(profile_cont, profile_block);
                    }
                    let profile_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, profile_enabled_val, 0);
                    builder
                        .ins()
                        .brif(profile_bool, profile_block, &[], profile_cont, &[]);
                    builder.switch_to_block(profile_block);
                    builder.seal_block(profile_block);
                    builder.ins().call(local_profile_struct, &[]);
                    builder.ins().jump(profile_cont, &[]);
                    builder.switch_to_block(profile_cont);
                    builder.seal_block(profile_cont);
                    let old_val = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), obj_ptr, offset);
                    let old_is_ptr = is_ptr_tag(&mut builder, old_val);
                    let new_is_ptr = is_ptr_tag(&mut builder, *val);
                    let either_ptr = builder.ins().bor(old_is_ptr, new_is_ptr);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    let store_block = builder.create_block();
                    let cont_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(fast_block, current_block);
                        builder.insert_block_after(slow_block, fast_block);
                        builder.insert_block_after(store_block, slow_block);
                        builder.insert_block_after(cont_block, store_block);
                    }

                    builder
                        .ins()
                        .brif(either_ptr, slow_block, &[], fast_block, &[]);

                    builder.switch_to_block(fast_block);
                    builder.seal_block(fast_block);
                    builder.ins().store(MemFlags::new(), *val, obj_ptr, offset);
                    builder.ins().jump(cont_block, &[]);

                    builder.switch_to_block(slow_block);
                    builder.seal_block(slow_block);
                    emit_mark_has_ptrs(&mut builder, obj_ptr);
                    let is_same = builder.ins().icmp(IntCC::Equal, old_val, *val);
                    builder
                        .ins()
                        .brif(is_same, cont_block, &[], store_block, &[]);

                    builder.switch_to_block(store_block);
                    builder.seal_block(store_block);
                    emit_maybe_ref_adjust(&mut builder, old_val, local_dec_ref_obj);
                    emit_maybe_ref_adjust(&mut builder, *val, local_inc_ref_obj);
                    builder.ins().store(MemFlags::new(), *val, obj_ptr, offset);
                    builder.ins().jump(cont_block, &[]);

                    builder.switch_to_block(cont_block);
                    builder.seal_block(cont_block);
                    if let Some(out_name) = op.out.as_ref() {
                        if out_name != "none" {
                            let res = builder.ins().iconst(types::I64, box_none());
                            vars.insert(out_name.clone(), res);
                        }
                    }
                }
                "store_init" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let val = vars.get(&args[1]).expect("Value not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let new_is_ptr = is_ptr_tag(&mut builder, *val);
                    let mark_block = builder.create_block();
                    let cont_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(mark_block, current_block);
                        builder.insert_block_after(cont_block, mark_block);
                    }
                    builder
                        .ins()
                        .brif(new_is_ptr, mark_block, &[], cont_block, &[]);

                    builder.switch_to_block(mark_block);
                    builder.seal_block(mark_block);
                    emit_mark_has_ptrs(&mut builder, obj_ptr);
                    builder.ins().jump(cont_block, &[]);

                    builder.switch_to_block(cont_block);
                    builder.seal_block(cont_block);
                    builder.ins().store(MemFlags::new(), *val, obj_ptr, offset);
                    if let Some(out_name) = op.out.as_ref() {
                        if out_name != "none" {
                            let res = builder.ins().iconst(types::I64, box_none());
                            vars.insert(out_name.clone(), res);
                        }
                    }
                }
                "load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), obj_ptr, offset);
                    emit_maybe_ref_adjust(&mut builder, res, local_inc_ref_obj);
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "closure_load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_closure_load", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, offset]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "closure_store" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let val = vars.get(&args[1]).expect("Value not found");
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_closure_store", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, offset, *val]);
                    if let Some(out_name) = op.out {
                        let res = builder.inst_results(call)[0];
                        vars.insert(out_name, res);
                    }
                }
                "guarded_load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), obj_ptr, offset);
                    emit_maybe_ref_adjust(&mut builder, res, local_inc_ref_obj);
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "guarded_field_get" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let expected_version = vars.get(&args[2]).expect("Expected version not found");
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guarded_field_get_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            obj_ptr,
                            *class_bits,
                            *expected_version,
                            offset,
                            attr_ptr,
                            attr_len,
                        ],
                    );
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "guarded_field_set" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let expected_version = vars.get(&args[2]).expect("Expected version not found");
                    let val = vars.get(&args[3]).expect("Value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guarded_field_set_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            obj_ptr,
                            *class_bits,
                            *expected_version,
                            offset,
                            *val,
                            attr_ptr,
                            attr_len,
                        ],
                    );
                    if let Some(out_name) = op.out.as_ref() {
                        if out_name != "none" {
                            let res = builder.inst_results(call)[0];
                            vars.insert(out_name.clone(), res);
                        }
                    }
                }
                "guarded_field_init" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits = vars.get(&args[1]).expect("Class not found");
                    let expected_version = vars.get(&args[2]).expect("Expected version not found");
                    let val = vars.get(&args[3]).expect("Value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guarded_field_init_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            obj_ptr,
                            *class_bits,
                            *expected_version,
                            offset,
                            *val,
                            attr_ptr,
                            attr_len,
                        ],
                    );
                    if let Some(out_name) = op.out.as_ref() {
                        if out_name != "none" {
                            let res = builder.inst_results(call)[0];
                            vars.insert(out_name.clone(), res);
                        }
                    }
                }
                "guard_type" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Guard value not found");
                    let expected = vars.get(&args[1]).expect("Guard expected tag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guard_type", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*val, *expected]);
                }
                "guard_layout" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Guard object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits = vars.get(&args[1]).expect("Guard class not found");
                    let expected_version = vars.get(&args[2]).expect("Guard version not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guard_layout_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, *class_bits, *expected_version]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "get_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "get_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_object", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "get_attr_special_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_special", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "get_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = vars.get(&args[1]).expect("Attr name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "get_attr_name_default" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = vars.get(&args[1]).expect("Attr name not found");
                    let default = vars.get(&args[2]).expect("Attr default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_name_default", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name, *default]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "has_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = vars.get(&args[1]).expect("Attr name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_has_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = vars.get(&args[1]).expect("Attr name not found");
                    let val = vars.get(&args[2]).expect("Attr value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let val = vars.get(&args[1]).expect("Attr value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_attr_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "set_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val = vars.get(&args[1]).expect("Attr value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_attr_object", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len, *val]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "del_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_attr_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "del_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let attr_name = op.s_value.as_ref().unwrap();
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_attr_object", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "del_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = vars.get(&args[1]).expect("Attr name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "ret" => {
                    let var_name = op.var.as_ref().unwrap();
                    let ret_val = *vars.get(var_name).expect("Return variable not found");
                    if let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup =
                                drain_cleanup_tracked(names, &last_use, op_idx, Some(var_name));
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup =
                                drain_cleanup_tracked(names, &last_use, op_idx, Some(var_name));
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref, &[tracked.value]);
                            }
                        }
                    }
                    tracked_vars.retain(|v| v != var_name);
                    tracked_obj_vars.retain(|v| v != var_name);
                    if has_ret {
                        builder.ins().jump(master_return_block, &[ret_val]);
                    } else {
                        builder.ins().jump(master_return_block, &[]);
                    }
                    is_block_filled = true;
                }
                "ret_void" => {
                    if let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref, &[tracked.value]);
                            }
                        }
                    }
                    if has_ret {
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().jump(master_return_block, &[zero]);
                    } else {
                        builder.ins().jump(master_return_block, &[]);
                    }
                    is_block_filled = true;
                }
                "jump" => {
                    let target_id = op.value.unwrap();
                    let target_block = state_blocks[&target_id];
                    if let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref, &[tracked.value]);
                            }
                        }
                    }
                    builder.ins().jump(target_block, &[]);
                    is_block_filled = true;
                }
                "br_if" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = vars.get(&args[0]).expect("Cond not found");
                    let target_id = op.value.unwrap();
                    let target_block = state_blocks[&target_id];

                    let fallthrough_block = builder.create_block();
                    // Note: In Molt IR, cond is 0 for false, !=0 for true.
                    // But brif takes a boolean condition (i32/i8 depending on type, Cranelift uses comparison result).
                    // We assume cond is already a boolean-like from cmp or we compare it to 0.
                    // Wait, `cond` from `vars` is I64 (NaN-boxed or raw int).
                    // We should check if it's truthy.
                    // But for now let's assume the frontend emits a boolean comparison result (0 or 1).
                    // Actually, let's play safe and check != 0.
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, *cond, 0);

                    builder
                        .ins()
                        .brif(cond_bool, target_block, &[], fallthrough_block, &[]);

                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(fallthrough_block, current_block);
                    }
                    builder.switch_to_block(fallthrough_block);
                    builder.seal_block(fallthrough_block);
                }
                "label" | "state_label" => {
                    let label_id = op.value.unwrap();
                    let block = state_blocks[&label_id];

                    if !is_block_filled {
                        builder.ins().jump(block, &[]);
                    }

                    builder.switch_to_block(block);
                    builder.seal_block(block);
                    is_block_filled = false;
                }
                "phi" => {}
                _ => {}
            }

            if let Some(name) = out_name {
                if name != "none" {
                    if let Some(block) = builder.current_block() {
                        if block == entry_block && loop_depth == 0 {
                            if output_is_ptr {
                                tracked_vars.push(name);
                            } else {
                                tracked_obj_vars.push(name);
                            }
                        } else if let Some(val) = vars.get(&name) {
                            let tracked = TrackedValue {
                                name: name.to_string(),
                                value: *val,
                            };
                            if output_is_ptr {
                                block_tracked_ptr.entry(block).or_default().push(tracked);
                            } else {
                                block_tracked_obj.entry(block).or_default().push(tracked);
                            }
                        }
                    }
                }
            }
        }

        // Finalize Master Return Block
        if !is_block_filled {
            if has_ret {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().jump(master_return_block, &[zero]);
            } else {
                builder.ins().jump(master_return_block, &[]);
            }
        }

        builder.switch_to_block(master_return_block);
        builder.seal_block(master_return_block);

        let final_res = if has_ret {
            let res = builder.block_params(master_return_block)[0];
            Some(res)
        } else {
            None
        };

        // Cleanup: DecRef tracked vars
        for name in tracked_vars {
            if let Some(val) = vars.get(&name) {
                builder.ins().call(local_dec_ref, &[*val]);
            }
        }

        for name in tracked_obj_vars {
            if let Some(val) = vars.get(&name) {
                builder.ins().call(local_dec_ref_obj, &[*val]);
            }
        }

        if let Some(res) = final_res {
            builder.ins().return_(&[res]);
        } else {
            builder.ins().return_(&[]);
        }

        builder.seal_all_blocks();
        builder.finalize();

        if let Some(config) = should_dump_ir() {
            if dump_ir_matches(&config, &func_ir.name) {
                dump_ir_ops(&func_ir, &config.mode);
            }
        }

        if let Ok(filter) = std::env::var("MOLT_DUMP_CLIF") {
            if filter == "1" || filter == func_ir.name || func_ir.name.contains(&filter) {
                eprintln!("CLIF {}:\n{}", func_ir.name, self.ctx.func.display());
            }
        }

        let id = self
            .module
            .declare_function(&func_ir.name, Linkage::Export, &self.ctx.func.signature)
            .unwrap();
        if let Err(err) = self.module.define_function(id, &mut self.ctx) {
            let err_text = format!("{err:?}");
            eprintln!(
                "Backend verification failed in {}: {err_text}",
                func_ir.name
            );
            if let Some(config) = should_dump_ir() {
                if dump_ir_matches(&config, &func_ir.name) {
                    dump_ir_ops(&func_ir, &config.mode);
                }
            }
            if let Ok(flag) = std::env::var("MOLT_DUMP_CLIF_ON_ERROR") {
                let clif = self.ctx.func.display().to_string();
                if let Some(inst) = parse_inst_id(&err_text) {
                    let needle = format!("inst{inst}");
                    let lines: Vec<&str> = clif.lines().collect();
                    let mut hit = None;
                    for (idx, line) in lines.iter().enumerate() {
                        if line.contains(&needle) {
                            hit = Some(idx);
                            break;
                        }
                    }
                    if let Some(center) = hit {
                        let start = center.saturating_sub(3);
                        let end = (center + 3).min(lines.len().saturating_sub(1));
                        eprintln!("CLIF snippet for {} around {}:", func_ir.name, needle);
                        for (offset, line) in lines[start..=end].iter().enumerate() {
                            let idx = start + offset;
                            eprintln!("{:04}: {}", idx + 1, line);
                        }
                    } else if flag == "full" {
                        eprintln!("CLIF {}:\n{}", func_ir.name, clif);
                    }
                } else if flag == "full" {
                    eprintln!("CLIF {}:\n{}", func_ir.name, clif);
                }
            }
            panic!("Backend compilation failed");
        }
        self.module.clear_context(&mut self.ctx);
    }
}
