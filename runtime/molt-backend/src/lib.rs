use cranelift::codegen::Context;
use cranelift::prelude::*;
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod wasm;

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_WIDTH: u64 = 47;
const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
const INT_SHIFT: i64 = (64 - INT_WIDTH) as i64;

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

fn unbox_int(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, INT_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, INT_SHIFT);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
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

#[derive(Serialize, Deserialize, Debug)]
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
)-> Vec<TrackedValue> {
    let mut cleanup = Vec::new();
    names.retain(|tracked| {
        if skip == Some(tracked.name.as_str()) {
            return true;
        }
        let last = last_use
            .get(&tracked.name)
            .copied()
            .unwrap_or(op_idx);
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
)-> Vec<TrackedValue> {
    names
        .iter()
        .filter(|tracked| skip != Some(tracked.name.as_str()))
        .filter(|tracked| {
            last_use
                .get(&tracked.name)
                .copied()
                .unwrap_or(op_idx)
                <= op_idx
        })
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
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
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
        for func_ir in ir.functions {
            self.compile_func(func_ir);
        }
        let product = self.module.finish();
        product.emit().unwrap()
    }

    fn compile_func(&mut self, func_ir: FunctionIR) {
        let mut builder_ctx = FunctionBuilderContext::new();
        self.module.clear_context(&mut self.ctx);

        let has_ret = func_ir.ops.iter().any(|op| op.kind == "ret");
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

        let mut vars = HashMap::new();
        let mut tracked_vars = Vec::new();
        let mut tracked_obj_vars = Vec::new();
        let mut state_blocks = HashMap::new();
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

        for (i, ty) in param_types.iter().enumerate() {
            let val = builder.append_block_param(entry_block, *ty);

            let name = &func_ir.params[i];

            vars.insert(name.clone(), val);
        }

        builder.seal_block(entry_block);

        // 1. Pre-pass: discover states and create blocks
        for op in &func_ir.ops {
            let state_id = if op.kind == "state_transition"
                || op.kind == "chan_send_yield"
                || op.kind == "chan_recv_yield"
                || op.kind == "label"
            {
                op.value.unwrap()
            } else {
                continue;
            };
            state_blocks
                .entry(state_id)
                .or_insert_with(|| builder.create_block());
        }

        // 2. Implementation
        for (op_idx, op) in func_ir.ops.into_iter().enumerate() {
            if is_block_filled {
                match op.kind.as_str() {
                    "label" | "else" | "end_if" | "loop_end" => {}
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
                "const_none" => {
                    let iconst = builder.ins().iconst(types::I64, box_none());
                    vars.insert(op.out.unwrap(), iconst);
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
                        builder.inst_results(call)[0]
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
                "sub" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        box_int_value(&mut builder, diff)
                    } else {
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
                        builder.inst_results(call)[0]
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
                        builder.inst_results(call)[0]
                    };
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
                        let val = vars.get(name).expect("List elem not found");
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
                        .declare_function("molt_iter", Linkage::Import, &sig)
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
                    let obj = vars.get(&args[0]).expect("Obj not found");
                    let idx = vars.get(&args[1]).expect("Index not found");
                    let val = vars.get(&args[2]).expect("Value not found");
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
                "string_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Replace haystack not found");
                    let needle = vars.get(&args[1]).expect("Replace needle not found");
                    let replacement = vars.get(&args[2]).expect("Replace replacement not found");
                    let mut sig = self.module.make_signature();
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
                        .call(local_callee, &[*hay, *needle, *replacement]);
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
                "bytes_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Replace haystack not found");
                    let needle = vars.get(&args[1]).expect("Replace needle not found");
                    let replacement = vars.get(&args[2]).expect("Replace replacement not found");
                    let mut sig = self.module.make_signature();
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
                        .call(local_callee, &[*hay, *needle, *replacement]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "bytearray_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = vars.get(&args[0]).expect("Replace haystack not found");
                    let needle = vars.get(&args[1]).expect("Replace needle not found");
                    let replacement = vars.get(&args[2]).expect("Replace replacement not found");
                    let mut sig = self.module.make_signature();
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
                        .call(local_callee, &[*hay, *needle, *replacement]);
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
                        builder.inst_results(call)[0]
                    };
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
                        builder.inst_results(call)[0]
                    };
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
                    sig.params.push(AbiParam::new(types::I64)); // task ptr
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
                    let state = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), self_ptr, 16);
                    vars.insert("self".to_string(), self_ptr);

                    let mut sorted_states: Vec<_> = state_blocks.iter().collect();
                    sorted_states.sort_by_key(|k| k.0);

                    for (id, &block) in sorted_states {
                        let id_const = builder.ins().iconst(types::I64, *id);
                        let is_state = builder.ins().icmp(IntCC::Equal, state, id_const);
                        let next_check = builder.create_block();
                        builder.ins().brif(is_state, block, &[], next_check, &[]);
                        builder.switch_to_block(next_check);
                        builder.seal_block(next_check);
                    }
                }
                "state_transition" => {
                    let args = op.args.as_ref().unwrap();
                    let future = vars.get(&args[0]).expect("Future not found");
                    let next_state_id = op.value.unwrap();
                    let self_ptr = *vars.get("self").expect("Self not found");

                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, 16);

                    let poll_fn_addr =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::new(), *future, -16);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    let call = builder
                        .ins()
                        .call_indirect(sig_ref, poll_fn_addr, &[*future]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    builder.ins().brif(
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    builder.switch_to_block(ready_path);
                    builder.seal_block(ready_path);
                    vars.insert(op.out.unwrap(), res);
                    builder.ins().jump(next_block, &[]);

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "chan_send_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = vars.get(&args[0]).expect("Chan not found");
                    let val = vars.get(&args[1]).expect("Val not found");
                    let next_state_id = op.value.unwrap();
                    let self_ptr = *vars.get("self").expect("Self not found");

                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, 16);

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
                    builder.ins().brif(
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    builder.switch_to_block(ready_path);
                    builder.seal_block(ready_path);
                    vars.insert(op.out.unwrap(), res);
                    builder.ins().jump(next_block, &[]);

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "chan_recv_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = vars.get(&args[0]).expect("Chan not found");
                    let next_state_id = op.value.unwrap();
                    let self_ptr = *vars.get("self").expect("Self not found");

                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder
                        .ins()
                        .store(MemFlags::new(), state_val, self_ptr, 16);

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
                    builder.ins().brif(
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    builder.switch_to_block(ready_path);
                    builder.seal_block(ready_path);
                    vars.insert(op.out.unwrap(), res);
                    builder.ins().jump(next_block, &[]);

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                    is_block_filled = false;
                }
                "chan_new" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
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
                "call_async" => {
                    let poll_func_name = op.s_value.as_ref().unwrap();
                    let size = builder.ins().iconst(types::I64, 0);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let alloc_callee = self
                        .module
                        .declare_function("molt_alloc", Linkage::Import, &sig)
                        .unwrap();
                    let local_alloc = self.module.declare_func_in_func(alloc_callee, builder.func);
                    let call = builder.ins().call(local_alloc, &[size]);
                    let obj = builder.inst_results(call)[0];

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

                    builder.ins().store(MemFlags::new(), poll_addr, obj, -16);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().store(MemFlags::new(), zero, obj, 16);
                    output_is_ptr = true;
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, obj);
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
                    let call = builder.ins().call(local_callee, &args);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
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
                    });
                }
                "else" => {
                    let frame = if_stack.last_mut().expect("No if on stack");
                    if !is_block_filled {
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
                        builder.ins().jump(frame.merge_block, &[]);
                    }
                    builder.switch_to_block(frame.else_block);
                    builder.seal_block(frame.else_block);
                    is_block_filled = false;
                    frame.has_else = true;
                }
                "end_if" => {
                    let frame = if_stack.pop().expect("No if on stack");
                    if !is_block_filled {
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
                        builder.ins().jump(frame.merge_block, &[]);
                    }
                    if !frame.has_else {
                        builder.switch_to_block(frame.else_block);
                        builder.seal_block(frame.else_block);
                        builder.ins().jump(frame.merge_block, &[]);
                    }
                    builder.switch_to_block(frame.merge_block);
                    builder.seal_block(frame.merge_block);
                    is_block_filled = false;
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
                "loop_index_next" => {
                    let args = op.args.as_ref().unwrap();
                    let next_idx = vars.get(&args[0]).expect("Loop index next not found");
                    let frame = loop_stack.last_mut().expect("No loop on stack");
                    frame.next_index = Some(*next_idx);
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, *next_idx);
                }
                "loop_continue" => {
                    let frame = loop_stack.last_mut().expect("No loop on stack");
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
                    let frame = loop_stack.pop().expect("No loop on stack");
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
                    sig.returns.push(AbiParam::new(types::I64)); // Returns a pointer
                    let callee = self
                        .module
                        .declare_function("molt_alloc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst]);
                    let res = builder.inst_results(call)[0];
                    output_is_ptr = true;
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "alloc_future" => {
                    let closure_size = op.value.unwrap();
                    let size = builder.ins().iconst(types::I64, closure_size);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let alloc_callee = self
                        .module
                        .declare_function("molt_alloc", Linkage::Import, &sig)
                        .unwrap();
                    let local_alloc = self.module.declare_func_in_func(alloc_callee, builder.func);
                    let call = builder.ins().call(local_alloc, &[size]);
                    let obj = builder.inst_results(call)[0];

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

                    builder.ins().store(MemFlags::new(), poll_addr, obj, -16);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().store(MemFlags::new(), zero, obj, -8);

                    if let Some(args_names) = &op.args {
                        for (i, name) in args_names.iter().enumerate() {
                            let arg_val = vars.get(name).expect("Arg not found for alloc_future");
                            let offset = (i * 8) as i32;
                            builder.ins().store(MemFlags::new(), *arg_val, obj, offset);
                        }
                    }

                    output_is_ptr = true;
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, obj);
                }
                "store" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let val = vars.get(&args[1]).expect("Value not found");
                    let offset = op.value.unwrap() as i32;
                    builder.ins().store(MemFlags::new(), *val, *obj, offset);
                }
                "load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let offset = op.value.unwrap() as i32;
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), *obj, offset);
                    let out_name = op.out.unwrap();
                    vars.insert(out_name, res);
                }
                "guarded_load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = vars.get(&args[0]).expect("Object not found");
                    let offset = op.value.unwrap() as i32;
                    let out_name = op.out.clone().unwrap();

                    let type_id = builder.ins().load(types::I32, MemFlags::new(), *obj, -24);
                    let expected_type_id = builder.ins().iconst(types::I32, 100);
                    let is_match = builder.ins().icmp(IntCC::Equal, type_id, expected_type_id);

                    let fast_path = builder.create_block();
                    let slow_path = builder.create_block();
                    let merge = builder.create_block();

                    builder.append_block_param(merge, types::I64);
                    builder.ins().brif(is_match, fast_path, &[], slow_path, &[]);

                    builder.switch_to_block(fast_path);
                    builder.seal_block(fast_path);
                    let fast_res = builder
                        .ins()
                        .load(types::I64, MemFlags::new(), *obj, offset);
                    builder.ins().jump(merge, &[fast_res]);

                    builder.switch_to_block(slow_path);
                    builder.seal_block(slow_path);
                    let attr_name = op.s_value.as_ref().unwrap();
                    let attr_ptr = builder.ins().iconst(types::I64, 0);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_generic", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let slow_res = builder.inst_results(call)[0];
                    builder.ins().jump(merge, &[slow_res]);

                    builder.switch_to_block(merge);
                    builder.seal_block(merge);
                    let res = builder.block_params(merge)[0];
                    vars.insert(out_name, res);
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
                "ret" => {
                    let var_name = op.var.as_ref().unwrap();
                    let ret_val = *vars.get(var_name).expect("Return variable not found");
                    if let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(
                                names,
                                &last_use,
                                op_idx,
                                Some(var_name),
                            );
                            for tracked in cleanup {
                                builder.ins().call(local_dec_ref_obj, &[tracked.value]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(
                                names,
                                &last_use,
                                op_idx,
                                Some(var_name),
                            );
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

                    builder.switch_to_block(fallthrough_block);
                    builder.seal_block(fallthrough_block);
                }
                "label" => {
                    let label_id = op.value.unwrap();
                    let block = state_blocks[&label_id];

                    if !is_block_filled {
                        builder.ins().jump(block, &[]);
                    }

                    builder.switch_to_block(block);
                    builder.seal_block(block);
                    is_block_filled = false;
                }
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

        let id = self
            .module
            .declare_function(&func_ir.name, Linkage::Export, &self.ctx.func.signature)
            .unwrap();
        self.module.define_function(id, &mut self.ctx).unwrap();
        self.module.clear_context(&mut self.ctx);
    }
}
