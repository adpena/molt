use cranelift::prelude::*;
use cranelift::codegen::Context;
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub mod wasm;

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
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
    pub s_value: Option<String>,
    pub var: Option<String>,
    pub args: Option<Vec<String>>,
    pub out: Option<String>,
}

pub struct SimpleBackend {
    module: ObjectModule,
    ctx: Context,
    block_stack: Vec<Block>,
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
        ).unwrap();
        let module = ObjectModule::new(builder);
        let ctx = module.make_context();

        Self { module, ctx, block_stack: Vec::new() }
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
            self.ctx.func.signature.returns.push(AbiParam::new(types::I64));
        }
        if func_ir.name.ends_with("_poll") {
             self.ctx.func.signature.params.push(AbiParam::new(types::I64));
        }

        let param_types: Vec<types::Type> = self.ctx.func.signature.params.iter().map(|p| p.value_type).collect();
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut builder_ctx);

        let mut vars = HashMap::new();
        let mut tracked_vars = Vec::new();
        let mut state_blocks = HashMap::new();
        let mut is_block_filled = false;

        let entry_block = builder.create_block();
        let master_return_block = builder.create_block();
        if has_ret {
            builder.append_block_param(master_return_block, types::I64);
        }

        builder.switch_to_block(entry_block);

                for (i, ty) in param_types.iter().enumerate() {

                    let val = builder.append_block_param(entry_block, *ty);

                    let name = &func_ir.params[i];

                    vars.insert(name.clone(), val);

                }

                builder.seal_block(entry_block);



        // 1. Pre-pass: discover states and create blocks
        for op in &func_ir.ops {
            if op.kind == "state_transition" || op.kind == "chan_send_yield" || op.kind == "chan_recv_yield" {
                let state_id = op.value.unwrap();
                if !state_blocks.contains_key(&state_id) {
                    state_blocks.insert(state_id, builder.create_block());
                }
            }
        }

        // 2. Implementation
        for op in func_ir.ops {
            if is_block_filled { continue; }

            match op.kind.as_str() {
                "const" => {
                    let val = op.value.unwrap();
                    let boxed = box_int(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    vars.insert(op.out.unwrap(), iconst);
                }
                "const_str" => {
                    let s = op.s_value.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let data_id = self.module.declare_data(
                        &format!("str_{}_{}", func_ir.name, out_name),
                        Linkage::Export,
                        false,
                        false,
                    ).unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(s.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, s.len() as i64);

                    vars.insert(format!("{}_ptr", out_name), ptr);
                    vars.insert(format!("{}_len", out_name), len);
                    vars.insert(out_name, ptr);
                }
                "add" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_add", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "sub" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_sub", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "mul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_mul", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "lt" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = vars.get(&args[0]).expect("LHS not found");
                    let rhs = vars.get(&args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_lt", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "print" => {
                    let args = op.args.as_ref().unwrap();
                    let val = vars.get(&args[0]).expect("Print value not found");

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_print_obj", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*val]);
                }
                "json_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    let ptr = vars.get(&format!("{}_ptr", arg_name)).or_else(|| vars.get(arg_name)).expect("String ptr not found");
                    let len = vars.get(&format!("{}_len", arg_name)).expect("String len not found");

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status

                    let out_slot = builder.create_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                    let callee = self.module.declare_function("molt_json_parse_scalar", Linkage::Import, &sig).unwrap();
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

                    let callee = self.module.declare_function("molt_block_on", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*task]);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "state_switch" => {
                    let self_ptr = builder.block_params(entry_block)[0];
                    let state = builder.ins().load(types::I64, MemFlags::new(), self_ptr, 16);
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
                    builder.ins().store(MemFlags::new(), state_val, self_ptr, 16);

                    let poll_fn_addr = builder.ins().load(types::I64, MemFlags::new(), *future, -16);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    let call = builder.ins().call_indirect(sig_ref, poll_fn_addr, &[*future]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, i64::from_ne_bytes(0x7ffc_0000_0000_0000u64.to_ne_bytes()));
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    builder.ins().brif(is_pending, master_return_block, &[pending_const], ready_path, &[]);

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
                    builder.ins().store(MemFlags::new(), state_val, self_ptr, 16);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_chan_send", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan, *val]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, i64::from_ne_bytes(0x7ffc_0000_0000_0000u64.to_ne_bytes()));
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    builder.ins().brif(is_pending, master_return_block, &[pending_const], ready_path, &[]);

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
                    builder.ins().store(MemFlags::new(), state_val, self_ptr, 16);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_chan_recv", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, i64::from_ne_bytes(0x7ffc_0000_0000_0000u64.to_ne_bytes()));
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    builder.ins().brif(is_pending, master_return_block, &[pending_const], ready_path, &[]);

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
                    let callee = self.module.declare_function("molt_chan_new", Linkage::Import, &sig).unwrap();
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
                    let callee = self.module.declare_function("molt_spawn", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*task]);
                }
                "call_async" => {
                    let poll_func_name = op.s_value.as_ref().unwrap();
                    let size = builder.ins().iconst(types::I64, 0);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let alloc_callee = self.module.declare_function("molt_alloc", Linkage::Import, &sig).unwrap();
                    let local_alloc = self.module.declare_func_in_func(alloc_callee, builder.func);
                    let call = builder.ins().call(local_alloc, &[size]);
                    let obj = builder.inst_results(call)[0];

                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));
                    let poll_func_id = self.module.declare_function(poll_func_name, Linkage::Import, &poll_sig).unwrap();
                    let poll_func_ref = self.module.declare_func_in_func(poll_func_id, builder.func);
                    let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                    builder.ins().store(MemFlags::new(), poll_addr, obj, -16);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().store(MemFlags::new(), zero, obj, 16);
                    let out_name = op.out.unwrap();
                    tracked_vars.push(out_name.clone());
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

                    let callee = self.module.declare_function(target_name, Linkage::Export, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &args);
                    let res = builder.inst_results(call)[0];
                    vars.insert(op.out.unwrap(), res);
                }
                "if_return" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = vars.get(&args[0]).expect("Cond not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self.module.declare_function("molt_is_truthy", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    builder.ins().brif(cond_bool, then_block, &[], else_block, &[]);
                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    self.block_stack.push(else_block);
                }
                "end_if" => {
                    let else_block = self.block_stack.pop().expect("No block on stack");
                    builder.switch_to_block(else_block);
                    builder.seal_block(else_block);
                }
                "alloc" => {
                    let size = op.value.unwrap();
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns a pointer
                    let callee = self.module.declare_function("molt_alloc", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    tracked_vars.push(out_name.clone());
                    vars.insert(out_name, res);
                }
                "alloc_future" => {
                    let closure_size = op.value.unwrap();
                    let size = builder.ins().iconst(types::I64, closure_size);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let alloc_callee = self.module.declare_function("molt_alloc", Linkage::Import, &sig).unwrap();
                    let local_alloc = self.module.declare_func_in_func(alloc_callee, builder.func);
                    let call = builder.ins().call(local_alloc, &[size]);
                    let obj = builder.inst_results(call)[0];

                    let poll_func_name = op.s_value.as_ref().unwrap();
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));

                    let poll_func_id = self.module.declare_function(poll_func_name, Linkage::Export, &poll_sig).unwrap();
                    let poll_func_ref = self.module.declare_func_in_func(poll_func_id, builder.func);
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

                    let out_name = op.out.unwrap();
                    tracked_vars.push(out_name.clone());
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
                    let res = builder.ins().load(types::I64, MemFlags::new(), *obj, offset);
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
                    let fast_res = builder.ins().load(types::I64, MemFlags::new(), *obj, offset);
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
                    let callee = self.module.declare_function("molt_get_attr_generic", Linkage::Import, &sig).unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let slow_res = builder.inst_results(call)[0];
                    builder.ins().jump(merge, &[slow_res]);

                    builder.switch_to_block(merge);
                    builder.seal_block(merge);
                    let res = builder.block_params(merge)[0];
                    vars.insert(out_name, res);
                }
                "ret" => {
                    let var_name = op.var.as_ref().unwrap();
                    let ret_val = *vars.get(var_name).expect("Return variable not found");
                    tracked_vars.retain(|v| v != var_name);
                    if has_ret {
                        builder.ins().jump(master_return_block, &[ret_val]);
                    } else {
                        builder.ins().jump(master_return_block, &[]);
                    }
                    is_block_filled = true;
                }
                "ret_void" => {
                    if has_ret {
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().jump(master_return_block, &[zero]);
                    } else {
                        builder.ins().jump(master_return_block, &[]);
                    }
                    is_block_filled = true;
                }
                _ => {}
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
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let dec_ref_callee = self.module.declare_function("molt_dec_ref", Linkage::Import, &sig).unwrap();
        let local_dec_ref = self.module.declare_func_in_func(dec_ref_callee, builder.func);
        for name in tracked_vars {
            if let Some(val) = vars.get(&name) {
                builder.ins().call(local_dec_ref, &[*val]);
            }
        }

        if let Some(res) = final_res {
            builder.ins().return_(&[res]);
        } else {
            builder.ins().return_(&[]);
        }

        builder.finalize();

        let id = self.module
            .declare_function(&func_ir.name, Linkage::Export, &self.ctx.func.signature)
            .unwrap();
        self.module.define_function(id, &mut self.ctx).unwrap();
        self.module.clear_context(&mut self.ctx);
    }
}
