//! LLVM projection of the application's immutable dense slot bindings.

#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use inkwell::{module::Linkage, values::GlobalValue};
#[cfg(feature = "llvm")]
use molt_ir::code_slots::{CodeSlotBindings, ProgramSlotKind, program_slot_symbol};

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

    pub(crate) fn emit_program_slot_bindings(
        &self,
        code_slots: Option<&CodeSlotBindings>,
        modules: Option<&molt_ir::ModuleRegistryIR>,
    ) {
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
    }
}
