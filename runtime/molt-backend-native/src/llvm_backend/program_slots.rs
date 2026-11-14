//! LLVM projection of the application's immutable dense slot bindings.

#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use inkwell::{module::Linkage, values::GlobalValue};
#[cfg(feature = "llvm")]
use molt_ir::code_slots::{
    CodeSlotBindings, ProgramSlotKind, program_slot_symbol, static_cell_block_symbol,
};

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub(crate) fn program_slot_binding(
        &self,
        kind: ProgramSlotKind,
        owner: &str,
    ) -> GlobalValue<'ctx> {
        let symbol = program_slot_symbol(kind, owner);
        let global = self.module.get_global(&symbol).unwrap_or_else(|| {
            self.module
                .add_global(self.context.i64_type(), None, &symbol)
        });
        global.set_linkage(Linkage::External);
        global.set_constant(true);
        global.set_alignment(8);
        global
    }

    /// A cell-eligible module's static cell block (call_anatomy.md 4.5a), as
    /// a function object imports it: the program object defines it.
    pub(crate) fn static_cell_block(&self, module: &str) -> GlobalValue<'ctx> {
        let symbol = static_cell_block_symbol(module);
        self.module.get_global(&symbol).unwrap_or_else(|| {
            let global = self
                .module
                .add_global(self.context.i64_type(), None, &symbol);
            global.set_linkage(Linkage::External);
            global.set_constant(false);
            global.set_alignment(8);
            global
        })
    }

    /// Define every static cell block once, in the program object: one
    /// writable word per layout name, each initially unbound.
    fn emit_static_cell_blocks(&self, modules: &molt_ir::ModuleRegistryIR) {
        let i64_type = self.context.i64_type();
        let unbound = i64_type.const_int(molt_codegen_abi::NAMESPACE_SLOT_UNBOUND, false);
        for (module, count) in modules.static_cell_blocks() {
            let symbol = static_cell_block_symbol(module);
            let ty = i64_type.array_type(count);
            let words = vec![unbound; count as usize];
            let block = self.module.add_global(ty, None, "__molt_static_cells_new");
            block.set_linkage(Linkage::External);
            block.set_constant(false);
            block.set_alignment(8);
            block.set_initializer(&i64_type.const_array(&words));
            if let Some(declared) = self.module.get_global(&symbol) {
                // A function of this object imported the block as a word.
                declared
                    .as_pointer_value()
                    .replace_all_uses_with(block.as_pointer_value());
                unsafe { declared.delete() };
            }
            block.as_pointer_value().set_name(&symbol);
        }
    }

    pub(crate) fn emit_program_slot_bindings(
        &self,
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
            let global = self.program_slot_binding(kind, owner);
            global.set_initializer(&self.context.i64_type().const_int(slot, false));
        }
        if let Some(registry) = registry {
            self.emit_static_cell_blocks(registry);
        }
    }
}
