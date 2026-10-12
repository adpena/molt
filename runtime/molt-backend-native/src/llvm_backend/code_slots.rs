//! LLVM projection of the application's immutable dense slot bindings.

#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use inkwell::{module::Linkage, values::GlobalValue};
#[cfg(feature = "llvm")]
use molt_ir::code_slots::{CodeSlotBindings, code_slot_symbol};

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub(crate) fn code_slot_binding(&self, owner: &str) -> GlobalValue<'ctx> {
        let symbol = code_slot_symbol(owner);
        let global = self.module.get_global(&symbol).unwrap_or_else(|| {
            self.module
                .add_global(self.context.i64_type(), None, &symbol)
        });
        global.set_linkage(Linkage::External);
        global.set_constant(true);
        global.set_alignment(8);
        global
    }

    pub(crate) fn emit_code_slot_bindings(&self, bindings: &CodeSlotBindings) {
        for (owner, slot) in bindings.iter() {
            let global = self.code_slot_binding(owner);
            global.set_initializer(&self.context.i64_type().const_int(slot, false));
        }
    }
}
