//! Cranelift's spelling of the shared method site generators (call_anatomy.md
//! 4.5 "Methods"; `molt_codegen_abi::method_site` owns the meaning): inline
//! in the function being compiled, against a record in writable local data.

use super::*;
use molt_codegen_abi::method_site::{
    MethodSiteBuilder, generate_method_site_call, generate_method_site_load,
    method_site_record_size, write_method_site_record_image,
};

/// What a method site's inline paths call besides the frame primitives.
#[derive(Clone, Copy)]
pub(crate) struct NativeMethodSiteSites {
    pub(crate) frame: NativeFrameSites,
    /// `molt_recursion_enter_fast` and `molt_recursion_exit_fast`.
    pub(crate) recursion_enter: FuncRef,
    pub(crate) recursion_exit: FuncRef,
}

struct NativeMethodSiteBuilder<'a, 'b> {
    frames: NativeFrameBuilder<'a, 'b>,
    recursion_enter: FuncRef,
    recursion_exit: FuncRef,
}

impl<'a, 'b> molt_codegen_abi::word::WordBuilder for NativeMethodSiteBuilder<'a, 'b> {
    molt_codegen_abi::forward_word_builder!(frames: NativeFrameBuilder<'a, 'b>);
}

impl molt_codegen_abi::frame_record::FrameBuilder for NativeMethodSiteBuilder<'_, '_> {
    type Local = Variable;

    fn local(&mut self) -> Variable {
        self.frames.local()
    }
    fn set(&mut self, local: Variable, value: Value) {
        self.frames.set(local, value)
    }
    fn get(&mut self, local: Variable) -> Value {
        self.frames.get(local)
    }
    fn load_word(&mut self, address: Value, offset: i32) -> Value {
        self.frames.load_word(address, offset)
    }
    fn store_word(&mut self, address: Value, offset: i32, value: Value) {
        self.frames.store_word(address, offset, value)
    }
    fn load_header_u32(&mut self, pointer: Value, offset: i32) -> Value {
        self.frames.load_header_u32(pointer, offset)
    }
    fn add(&mut self, left: Value, right: Value) -> Value {
        self.frames.add(left, right)
    }
    fn retain(&mut self, bits: Value) {
        self.frames.retain(bits)
    }
    fn release(&mut self, bits: Value) {
        self.frames.release(bits)
    }
    fn enter_slow(&mut self, code_slot: Value) -> Value {
        self.frames.enter_slow(code_slot)
    }
    fn exit_slow(&mut self, base: Value) {
        self.frames.exit_slow(base)
    }
    fn attempt(&mut self, fast: impl FnOnce(&mut Self), slow: impl FnOnce(&mut Self)) {
        // The frame builder's attempt, run on this wrapper: the same blocks.
        let slow_block = self.frames.words.builder.create_block();
        let done = self.frames.words.builder.create_block();
        self.frames.words.builder.set_cold_block(slow_block);
        self.frames.slow_blocks.push(slow_block);
        fast(self);
        self.frames.slow_blocks.pop();
        self.frames.words.builder.ins().jump(done, &[]);
        enter_private_block(self.frames.words.builder, slow_block);
        slow(self);
        self.frames.words.builder.ins().jump(done, &[]);
        enter_private_block(self.frames.words.builder, done);
    }
    fn require(&mut self, condition: Value) {
        self.frames.require(condition)
    }
    fn if_then(&mut self, condition: Value, then: impl FnOnce(&mut Self)) {
        let then_block = self.frames.words.builder.create_block();
        let done = self.frames.words.builder.create_block();
        self.frames
            .words
            .builder
            .ins()
            .brif(condition, then_block, &[], done, &[]);
        enter_private_block(self.frames.words.builder, then_block);
        then(self);
        self.frames.words.builder.ins().jump(done, &[]);
        enter_private_block(self.frames.words.builder, done);
    }
    fn counted_loop(
        &mut self,
        _count: Value,
        _descending: bool,
        _body: impl FnMut(&mut Self, Value),
    ) {
        unreachable!("a method site has no loop")
    }
}

/// Every block a generator opens is private to it and entered once all its
/// predecessors exist.
fn enter_private_block(builder: &mut FunctionBuilder<'_>, block: Block) {
    builder.switch_to_block(block);
    builder.seal_block(block);
}

impl MethodSiteBuilder for NativeMethodSiteBuilder<'_, '_> {
    fn recursion_enter(&mut self) -> Value {
        let call = self
            .frames
            .words
            .builder
            .ins()
            .call(self.recursion_enter, &[]);
        self.frames.words.builder.inst_results(call)[0]
    }
    fn recursion_exit(&mut self) {
        self.frames
            .words
            .builder
            .ins()
            .call(self.recursion_exit, &[]);
    }
    fn call_entry(
        &mut self,
        entry: Value,
        self_word: Value,
        args: &[Value],
        defaults: &[Value],
    ) -> Value {
        let builder = &mut *self.frames.words.builder;
        // The fixed-arity entry: the execution context, then boxed words.
        let mut signature = cranelift_codegen::ir::Signature::new(builder.func.signature.call_conv);
        let count = 1 + args.len() + defaults.len();
        for _ in 0..=count {
            signature.params.push(AbiParam::new(types::I64));
        }
        signature.returns.push(AbiParam::new(types::I64));
        let signature = builder.import_signature(signature);
        let mut physical = Vec::with_capacity(count + 1);
        physical.push(compiled_context(builder));
        physical.push(self_word);
        physical.extend_from_slice(args);
        physical.extend_from_slice(defaults);
        let call = builder.ins().call_indirect(signature, entry, &physical);
        builder.inst_results(call)[0]
    }
}

/// The address of method site `site`'s record in `func_name`. The site's load
/// passes its attribute `name` and defines the record image; its call finds
/// the same record by name.
pub(crate) fn method_site_record(
    module: &mut dyn Module,
    builder: &mut FunctionBuilder<'_>,
    func_name: &str,
    site: u32,
    name: Option<&[u8]>,
) -> Value {
    let symbol = format!("__molt_method_site_{func_name}_{site}");
    let data = module
        .declare_data(&symbol, Linkage::Local, true, false)
        .unwrap_or_else(|error| panic!("method site record `{symbol}`: {error}"));
    if let Some(name) = name {
        let mut image = vec![0u8; method_site_record_size(name.len())];
        write_method_site_record_image(name, &mut image);
        let mut description = DataDescription::new();
        description.set_align(8);
        description.define(image.into_boxed_slice());
        match module.define_data(data, &description) {
            Ok(()) | Err(cranelift_module::ModuleError::DuplicateDefinition(_)) => {}
            Err(error) => panic!("method site record `{symbol}`: {error}"),
        }
    }
    let global = module.declare_data_in_func(data, builder.func);
    builder
        .ins()
        .symbol_value(module.target_config().pointer_type(), global)
}

/// A method load at the site whose record is `record`: the shared load body,
/// whose miss calls `cold` (`molt_method_site_load(record, receiver,
/// self_out)`).
pub(crate) fn emit_method_site_load(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeMethodSiteSites,
    record: Value,
    receiver: Value,
    self_out: Value,
    cold: FuncRef,
) -> Value {
    let mut emitter = NativeMethodSiteBuilder {
        frames: NativeFrameBuilder {
            words: NativeWords { builder },
            sites: sites.frame,
            slow_blocks: Vec::new(),
        },
        recursion_enter: sites.recursion_enter,
        recursion_exit: sites.recursion_exit,
    };
    generate_method_site_load(&mut emitter, record, receiver, self_out, |b| {
        let call = b
            .frames
            .words
            .builder
            .ins()
            .call(cold, &[record, receiver, self_out]);
        b.frames.words.builder.inst_results(call)[0]
    })
}

/// The call that consumes a method site's pair, `words` = [callable,
/// self_or_null, args...], all adopted: the shared call body, whose miss
/// calls `cold` (the runtime's loaded-method call) with the same words.
pub(crate) fn emit_method_site_call(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeMethodSiteSites,
    thread: Value,
    record: Value,
    words: &[Value],
    cold: FuncRef,
) -> Value {
    let mut emitter = NativeMethodSiteBuilder {
        frames: NativeFrameBuilder {
            words: NativeWords { builder },
            sites: sites.frame,
            slow_blocks: Vec::new(),
        },
        recursion_enter: sites.recursion_enter,
        recursion_exit: sites.recursion_exit,
    };
    generate_method_site_call(
        &mut emitter,
        thread,
        record,
        words[0],
        words[1],
        &words[2..],
        |b| {
            let call = b.frames.words.builder.ins().call(cold, words);
            b.frames.words.builder.inst_results(call)[0]
        },
    )
}
