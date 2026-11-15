//! WASM instruction selection for the shared list item read and store
//! (`molt_codegen_abi::list_item` owns the meaning).
//!
//! As the method site bodies are, the read is one generated body, `(context,
//! sequence, index, admission) -> item`, over an exact list or tuple, whose
//! miss calls `molt_index`, and the
//! store one body, `(context, list, index, value, admission) -> word`, whose
//! miss calls `molt_store_index`. A generic subscript site pushes its operands
//! and calls the body in place of the import.
use crate::SimpleIR;
use crate::wasm::WasmBackend;
use crate::wasm::method_site::emit_body;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use molt_codegen_abi::list_item::{
    ItemContainers, ListLocking, generate_list_item_get, generate_list_item_set,
};

/// Function indices of one module's generated list item bodies.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WasmListItemBodies {
    get: Option<u32>,
    set: Option<u32>,
}

impl WasmListItemBodies {
    /// The read body, which replaces the generic `molt_index` import.
    pub(crate) fn get(self) -> u32 {
        self.get
            .expect("a module with a subscript read emits its list read body")
    }

    /// The store body, which replaces the generic `molt_store_index` import.
    pub(crate) fn set(self) -> u32 {
        self.set
            .expect("a module with a subscript store emits its list store body")
    }
}

/// Declare and emit the module's list item bodies, beside the method site
/// bodies: after the RC bodies they call, before the callable-table layout.
pub(in crate::wasm) fn emit_list_item_bodies(
    backend: &mut WasmBackend,
    reloc: bool,
    ir: &SimpleIR,
) {
    let ops = || ir.functions.iter().flat_map(|function| function.ops.iter());
    let mut bodies = WasmListItemBodies::default();
    if ops().any(|op| op.kind == molt_ir::OpKind::Index) {
        // list, index, admission
        bodies.get = Some(emit_body(backend, reloc, 3, |b| {
            let (list, index) = (1, 2);
            // One body serves every read site, so it admits either kind.
            let containers = ItemContainers::ListsOrTuple;
            let item =
                generate_list_item_get(b, list, index, containers, ListLocking::BUILD, |b| {
                    b.push_get(list);
                    b.push_get(index);
                    let cold = b.imports[WasmRuntimeImport::Index];
                    emit_call(b.func, b.reloc, cold);
                    b.word()
                });
            b.push_get(item);
        }));
    }
    if ops().any(|op| op.kind == molt_ir::OpKind::StoreIndex) {
        // list, index, value, admission
        bodies.set = Some(emit_body(backend, reloc, 4, |b| {
            let (list, index, value) = (1, 2, 3);
            let stored =
                generate_list_item_set(b, list, index, value, ListLocking::BUILD, list, |b| {
                    b.push_get(list);
                    b.push_get(index);
                    b.push_get(value);
                    let cold = b.imports[WasmRuntimeImport::StoreIndex];
                    emit_call(b.func, b.reloc, cold);
                    b.word()
                });
            b.push_get(stored);
        }));
    }
    backend.import_ids.set_list_item_bodies(bodies);
}
