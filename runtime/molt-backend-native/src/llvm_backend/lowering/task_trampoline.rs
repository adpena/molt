use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use molt_tir::trampolines::TrampolineTaskKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Python constructor arguments and the poll's one raw frame parameter
    /// are different ABIs. The shared task kind/layout owns that distinction.
    pub(super) fn ensure_callable_target(
        &self,
        op: &TirOp,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> (FunctionValue<'ctx>, FunctionValue<'ctx>) {
        let metadata = match (op.attrs.get("task_kind"), op.attrs.get("task_closure_size")) {
            (None, None) => None,
            (Some(AttrValue::Str(kind)), Some(AttrValue::Int(size))) => Some((
                TrampolineTaskKind::from_constructor_kind(kind)
                    .unwrap_or_else(|| panic!("unknown callable task_kind `{kind}` for {name}")),
                *size,
            )),
            _ => {
                panic!("{name} requires complete callable task_kind and task_closure_size metadata")
            }
        };
        let Some((kind, closure_size)) = metadata else {
            return (
                self.ensure_function_symbol(name, arity, has_closure),
                self.ensure_plain_trampoline(name, has_closure),
            );
        };
        let layout = crate::trampoline_task_layout(kind);
        layout.validate_closure_size(
            closure_size,
            arity,
            has_closure,
            crate::GENERATOR_CONTROL_BYTES,
        );
        let poll = self.ensure_function_symbol(name, 1, false);
        let trampoline_name = format!(
            "{name}__molt_llvm_task_trampoline_{arity}_{}{}_{closure_size}",
            usize::from(has_closure),
            kind.trampoline_kind().symbol_suffix(),
        );
        if let Some(trampoline) = self.backend.module.get_compiled_function(&trampoline_name) {
            return (poll, trampoline);
        }
        let i64_ty = self.backend.context.i64_type();
        let trampoline = self.backend.module.add_compiled_function(
            &trampoline_name,
            i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false),
            Some(inkwell::module::Linkage::Internal),
        );
        let builder = self.backend.context.create_builder();
        builder.position_at_end(self.backend.context.append_basic_block(trampoline, "entry"));
        use crate::codegen_ssa::CodegenBuilder;
        use crate::codegen_ssa::task::{TaskPlacement, emit_task_trampoline};
        let params = Default::default();
        let state = super::codegen_builder::LlvmFunctionState::new(true);
        let mut b = LlvmCodegen {
            words: LlvmWords {
                context: self.backend.context,
                builder: &builder,
            },
            backend: self.backend,
            params: &params,
            state: &state,
        };
        let poll_address = b.function_address(poll);
        let frame_bytes = b.poll_frame_bytes(name);
        let closure = has_closure.then(|| trampoline.get_nth_param(1).unwrap().into_int_value());
        let arguments = trampoline.get_nth_param(2).unwrap().into_int_value();
        emit_task_trampoline(
            &mut b,
            layout,
            poll_address,
            frame_bytes,
            closure,
            arguments,
            arity,
        );
        (poll, trampoline)
    }
}
