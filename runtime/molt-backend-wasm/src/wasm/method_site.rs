//! WASM instruction selection for compiled method sites (call_anatomy.md 4.5
//! "Methods"; `molt_codegen_abi::method_site` owns the meaning).
//!
//! As the frame and slot bodies are, the site's load is one generated body,
//! `(context, record, receiver, self_out, admission) -> callable`, and its
//! call is one body per argument count, `(context, record, callable, self,
//! args.., admission) -> result`. Each site's record is a mutable data
//! segment holding the generated record image; a site pushes its operands and
//! calls the body.
use std::collections::{BTreeMap, BTreeSet};

use crate::SimpleIR;
use crate::wasm::WasmBackend;
use crate::wasm::frame_record::{WasmBodyLayout, WasmFrameBuilder};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::{Function, emit_call, emit_call_indirect};
use crate::wasm_data::DataSegmentRef;
use molt_codegen_abi::method_site::{
    METHOD_SITE_MAX_DEFAULTS, MethodSiteBuilder, generate_method_site_call,
    generate_method_site_load, method_site_record_size, write_method_site_record_image,
};
use wasm_encoder::{Instruction, ValType};

/// The runtime's fixed-arity loaded-method calls, the cold path of a site
/// call with that many arguments. A call with more keeps the runtime call.
pub(in crate::wasm) const COLD_CALLS: [WasmRuntimeImport; 5] = [
    WasmRuntimeImport::CallLoadedMethod0,
    WasmRuntimeImport::CallLoadedMethod1,
    WasmRuntimeImport::CallLoadedMethod2,
    WasmRuntimeImport::CallLoadedMethod3,
    WasmRuntimeImport::CallLoadedMethod4,
];

/// Function indices of one module's generated method site bodies.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WasmMethodSiteBodies {
    load: Option<u32>,
    calls: BTreeMap<usize, u32>,
}

impl WasmMethodSiteBodies {
    pub(crate) fn load(&self) -> u32 {
        self.load
            .expect("a module with a method load emits its load body")
    }

    /// The call body for `nargs` arguments after `self`, if a site call with
    /// that many exists.
    pub(crate) fn call(&self, nargs: usize) -> Option<u32> {
        self.calls.get(&nargs).copied()
    }
}

/// The argument counts of the module's site calls that have a body.
fn call_arities(ir: &SimpleIR) -> BTreeSet<usize> {
    ir.functions
        .iter()
        .flat_map(|function| function.ops.iter())
        .filter(|op| op.kind == molt_ir::OpKind::CallLoadedMethod && op.method_site.is_some())
        .filter_map(|op| op.args.as_ref().map(|args| args.len().saturating_sub(2)))
        .filter(|&nargs| nargs < COLD_CALLS.len())
        .collect()
}

/// Declare and emit the module's method site bodies, after the RC bodies
/// they call and before the callable-table layout fixes user indices.
pub(in crate::wasm) fn emit_method_site_bodies(
    backend: &mut WasmBackend,
    reloc: bool,
    ir: &SimpleIR,
) {
    let has_loads = ir
        .functions
        .iter()
        .flat_map(|function| function.ops.iter())
        .any(|op| op.kind == molt_ir::OpKind::LoadMethod);
    let mut bodies = WasmMethodSiteBodies::default();
    if has_loads {
        // record, receiver, self_out, admission
        bodies.load = Some(emit_body(backend, reloc, 4, |b| {
            let (record, receiver, self_out) = (1, 2, 3);
            let callable = generate_method_site_load(b, record, receiver, self_out, |b| {
                b.push_get(record);
                b.push_get(receiver);
                b.push_get(self_out);
                let cold = b.imports[WasmRuntimeImport::MethodSiteLoad];
                emit_call(b.func, b.reloc, cold);
                b.word()
            });
            b.push_get(callable);
        }));
    }
    for nargs in call_arities(ir) {
        // The entry types the call's shapes take: self, the arguments and up
        // to the record's usable defaults, after the execution context.
        let entry_types: BTreeMap<usize, u32> = (0..=METHOD_SITE_MAX_DEFAULTS)
            .map(|appended| {
                let words = 1 + nargs + appended;
                let ty = backend
                    .types
                    .function(&vec![ValType::I64; words + 1], &[ValType::I64]);
                (words, ty)
            })
            .collect();
        // record, callable, self, args.., admission
        let body = emit_body(backend, reloc, 3 + nargs + 1, |b| {
            b.entry_types = entry_types;
            crate::wasm::pending_work::emit_thread_record(b.func);
            let thread = b.word();
            let (record, callable, self_word) = (1, 2, 3);
            let args: Vec<u32> = (4..4 + nargs as u32).collect();
            let result =
                generate_method_site_call(b, thread, record, callable, self_word, &args, |b| {
                    for word in 2..4 + nargs as u32 {
                        b.push_get(word);
                    }
                    let cold = b.imports[COLD_CALLS[nargs]];
                    emit_call(b.func, b.reloc, cold);
                    b.word()
                });
            b.push_get(result);
        });
        bodies.calls.insert(nargs, body);
    }
    backend.import_ids.set_method_site_bodies(bodies);
}

/// One attached body with `logical` i64 parameters (the RC admission last)
/// returning an i64 its `generate` leaves on the operand stack.
fn emit_body(
    backend: &mut WasmBackend,
    reloc: bool,
    logical: usize,
    generate: impl FnOnce(&mut WasmFrameBuilder<'_>),
) -> u32 {
    let physical = logical + 1;
    let type_idx = backend
        .types
        .function(&vec![ValType::I64; physical], &[ValType::I64]);
    let function_index = backend.func_count;
    std::rc::Rc::make_mut(&mut backend.execution_call_abis)
        .register_attached(function_index, &vec![ValType::I64; logical]);
    backend.funcs.function(type_idx);
    let mut func = Function::new_with_locals_types(WasmBodyLayout::scratch_locals());
    func.attach_context(physical, backend.execution_call_abis.clone());
    let mut b = WasmFrameBuilder::new(
        &mut func,
        reloc,
        &backend.import_ids,
        WasmBodyLayout {
            admission: logical as u32,
            first_word: physical as u32,
        },
    );
    generate(&mut b);
    assert!(b.closed(), "a method site body left a control label open");
    func.instruction(&Instruction::End);
    backend.codes.function(&func);
    backend.func_count += 1;
    function_index
}

impl MethodSiteBuilder for WasmFrameBuilder<'_> {
    fn recursion_enter(&mut self) -> u32 {
        let enter = self.imports[WasmRuntimeImport::RecursionEnterFast];
        emit_call(self.func, self.reloc, enter);
        self.word()
    }

    fn recursion_exit(&mut self) {
        let exit = self.imports[WasmRuntimeImport::RecursionExitFast];
        emit_call(self.func, self.reloc, exit);
    }

    fn call_entry(&mut self, entry: u32, self_word: u32, args: &[u32], defaults: &[u32]) -> u32 {
        let words = 1 + args.len() + defaults.len();
        let ty = *self
            .entry_types
            .get(&words)
            .expect("the call body declared this entry type");
        // The execution context is local 0 of every attached body.
        self.push(Instruction::LocalGet(0));
        self.push_get(self_word);
        for &word in args.iter().chain(defaults) {
            self.push_get(word);
        }
        // The entry is a function table index on wasm32.
        self.push_get(entry);
        self.push(Instruction::I32WrapI64);
        emit_call_indirect(self.func, self.reloc, ty, 0);
        self.word()
    }
}

impl WasmBackend {
    /// The record of method site `site` in the function at `func_index`: a
    /// mutable data segment with the record image of the site's load, which
    /// names the attribute; the site's call finds the same record.
    pub(in crate::wasm) fn method_site_record(
        &mut self,
        reloc: bool,
        func_index: u32,
        site: u32,
        name: Option<&[u8]>,
    ) -> DataSegmentRef {
        if let Some(&record) = self.method_site_records.get(&(func_index, site)) {
            return record;
        }
        let name = name.expect("a method site's load precedes its call");
        let mut image = vec![0u8; method_site_record_size(name.len())];
        write_method_site_record_image(name, &mut image);
        let record = self.add_data_segment_mutable(reloc, &image);
        self.method_site_records.insert((func_index, site), record);
        record
    }
}
