//! Application-owned dense IDs behind stable, immutable slot symbols.

use super::*;
use molt_ir::code_slots::{
    CodeSlotBindings, ProgramSlotKind, program_slot_symbol, static_cell_block_symbol,
};

impl SimpleBackend {
    /// The data the application's code and every native stdlib function share.
    /// The JIT launcher links this into the program's data image and opens it
    /// before loading compiled stdlib code. AOT emits it into the app object.
    pub fn emit_native_program_storage(
        target_triple: Option<&str>,
        code_slots: &CodeSlotBindings,
        registry: &molt_ir::ModuleRegistryIR,
    ) -> Vec<u8> {
        let mut backend = Self::new_with_target(target_triple);
        backend.emit_module_ready_binding(registry);
        backend.emit_program_slot_bindings(Some(code_slots), Some(registry));
        super::super::finalize_native_object(backend.module.into_object().finish())
            .emit()
            .expect("emit native program storage")
    }

    pub(super) fn emit_module_ready_binding(&mut self, registry: &molt_ir::ModuleRegistryIR) {
        // The ready block: writable zeroed words the runtime stores borrowed
        // module pointers into (IC2).
        let ready_id = self
            .module
            .declare_data(molt_ir::MODULE_READY_SYMBOL, Linkage::Export, true, false)
            .unwrap_or_else(|e| {
                panic!("failed to declare {}: {e:?}", molt_ir::MODULE_READY_SYMBOL)
            });
        let mut ready = DataDescription::new();
        ready.set_align(8);
        ready.define_zeroinit(registry.ready_block_bytes());
        self.module
            .define_data(ready_id, &ready)
            .unwrap_or_else(|e| panic!("failed to define {}: {e:?}", molt_ir::MODULE_READY_SYMBOL));
    }

    pub(in crate::native_backend::simple_backend) fn emit_program_slot_bindings(
        &mut self,
        code_slots: Option<&CodeSlotBindings>,
        modules: Option<&molt_ir::ModuleRegistryIR>,
    ) {
        let registry = modules;
        let codes = code_slots
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .map(|(owner, slot)| (ProgramSlotKind::Code, owner, slot));
        let modules = modules
            .into_iter()
            .flat_map(|registry| registry.module_slot_bindings())
            .map(|(owner, slot)| (ProgramSlotKind::Module, owner, slot));
        for (kind, owner, slot) in codes.chain(modules) {
            let symbol = program_slot_symbol(kind, owner);
            define_immutable_word(&mut self.module, &symbol, slot);
        }
        // Static cell blocks (call_anatomy.md 4.5a): one writable word per
        // layout name, initially unbound, defined once in the program object.
        for (owner, count) in registry
            .into_iter()
            .flat_map(|registry| registry.static_cell_blocks())
        {
            let symbol = static_cell_block_symbol(owner);
            let data = self
                .module
                .declare_data(&symbol, Linkage::Export, true, false)
                .unwrap_or_else(|error| panic!("failed to declare static cells {symbol}: {error}"));
            let word = match self.module.isa().endianness() {
                cranelift_codegen::ir::Endianness::Little => {
                    molt_codegen_abi::NAMESPACE_SLOT_UNBOUND.to_le_bytes()
                }
                cranelift_codegen::ir::Endianness::Big => {
                    molt_codegen_abi::NAMESPACE_SLOT_UNBOUND.to_be_bytes()
                }
            };
            let bytes: Vec<u8> = word.repeat(count as usize);
            let mut description = DataDescription::new();
            description.set_align(8);
            description.define(bytes.into_boxed_slice());
            self.module
                .define_data(data, &description)
                .unwrap_or_else(|error| panic!("failed to define static cells {symbol}: {error}"));
        }
    }
}
