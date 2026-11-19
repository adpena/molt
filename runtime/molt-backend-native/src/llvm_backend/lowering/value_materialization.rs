use super::*;
impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Match native/WASM invocation ownership: entry can fail before a frame
    /// exists, and every return (including suspension) closes only its own.
    pub(super) fn initialize_owned_execution_frame(
        &mut self,
        layout: crate::codegen_ssa::frame::FrameLayout,
    ) {
        use crate::codegen_ssa::builder::CodegenBuilder;
        use crate::codegen_ssa::frame::FrameEntry;
        assert!(self.owned_frame_entered.is_none());
        self.owned_frame_entered = crate::codegen_ssa::frame::initialize_owned_execution_frame(
            &mut self.codegen(),
            self.func.execution_context,
            || layout.entry.kind(),
        );
        let homes = match (layout.homes, layout.entry) {
            (None, _) => None,
            (Some(_), FrameEntry::Absent) => {
                Some(self.build_entry_i64_alloca("borrowed_frame_homes"))
            }
            (Some(_), _) => self.owned_frame_entered.map(|frame| frame.slot),
        };
        self.codegen().set_frame_homes(homes);
    }

    pub(super) fn lend_chunk_frame_homes(&mut self) {
        use crate::codegen_ssa::builder::CodegenBuilder;
        let Some(slot) = self.codegen().frame_homes() else {
            return;
        };
        let frame = self.owned_frame_entered;
        if frame.is_some_and(|owned| owned.slot == slot) {
            return;
        }
        let abi = require_function_linkage_abi(self.func, self.backend);
        let (mut cx, mut b) = self.walker();
        crate::codegen_ssa::frame::lend_homes(&mut b, slot, |b| {
            crate::codegen_ssa::returns::emit_return(&mut cx, b, &abi, None, frame.as_ref())
        });
    }

    /// Parameters and phis project the same producer facts as frozen linkage.
    pub(super) fn effective_block_arg_type(&self, id: ValueId, declared: &TirType) -> TirType {
        self.repr_facts.effective_value_type(id, declared)
    }

    // ── Box / Unbox ──

    /// The walker's context and builder for one op: they borrow disjoint
    /// parts of this function's state (the value table, and the builder with
    /// its block parameters and per-function state).
    pub(super) fn walker(
        &mut self,
    ) -> (
        crate::codegen_ssa::function::FunctionCx<'_, LlvmCodegen<'_, 'ctx>>,
        LlvmCodegen<'_, 'ctx>,
    ) {
        let codegen = LlvmCodegen {
            words: LlvmWords {
                context: self.backend.context,
                builder: &self.backend.builder,
            },
            backend: self.backend,
            params: &self.block_params,
            state: &self.state,
        };
        let cx = crate::codegen_ssa::function::FunctionCx {
            func: self.func,
            plan: &self.repr_facts,
            values: &mut self.values,
            diagnostics: &self.diagnostics,
            runtime: &self.backend.runtime_callable_symbols,
            linkage: &self.backend.function_linkage_abis,
            functions: &self.backend.function_table,
            call_sites: &self.call_site_counter,
        };
        (cx, codegen)
    }

    /// Lower `op` in `source` through the walker's dispatch table
    /// (`FunctionCx::walker_owned_op`) when the walker owns it.
    pub(super) fn walker_owned(&mut self, source: BlockId, op: &TirOp) -> bool {
        let func = self.func;
        let exception_targets = self
            .exception_targets
            .get_or_init(|| crate::tir::dominators::exception_labels_to_blocks(&func.label_id_map));
        let mut codegen = LlvmCodegen {
            words: LlvmWords {
                context: self.backend.context,
                builder: &self.backend.builder,
            },
            backend: self.backend,
            params: &self.block_params,
            state: &self.state,
        };
        let mut cx = crate::codegen_ssa::function::FunctionCx {
            func,
            plan: &self.repr_facts,
            values: &mut self.values,
            diagnostics: &self.diagnostics,
            runtime: &self.backend.runtime_callable_symbols,
            linkage: &self.backend.function_linkage_abis,
            functions: &self.backend.function_table,
            call_sites: &self.call_site_counter,
        };
        let at = crate::codegen_ssa::walk::Position {
            source,
            blocks: &self.block_map,
            exception_targets,
        };
        cx.walker_owned_op(&mut codegen, &at, op, self.owned_frame_entered)
    }

    #[cfg(test)]
    /// Run one walker family method on this function's walker context.
    pub(super) fn walker_op(
        &mut self,
        lower: impl for<'a> FnOnce(
            &mut crate::codegen_ssa::function::FunctionCx<'_, LlvmCodegen<'a, 'ctx>>,
            &mut LlvmCodegen<'a, 'ctx>,
        ),
    ) {
        let (mut cx, mut b) = self.walker();
        lower(&mut cx, &mut b);
    }

    /// One static entry-block word for an operation's out-parameter or owner
    /// slot; allocating where the operation runs would grow the stack on each
    /// loop iteration.
    pub(super) fn build_entry_i64_alloca(&self, name: &str) -> inkwell::values::PointerValue<'ctx> {
        self.codegen().entry_slot(name)
    }

    // ── Terminators ──

    /// The walker's view of this function's control flow.
    fn edges(&self) -> crate::codegen_ssa::control::Edges<'_, LlvmCodegen<'_, 'ctx>> {
        crate::codegen_ssa::control::Edges {
            func: self.func,
            plan: &self.repr_facts,
            values: &self.values,
            blocks: &self.block_map,
        }
    }

    fn record_fatals(&self, diagnostics: Vec<String>) {
        for diagnostic in diagnostics {
            self.record_fatal(diagnostic);
        }
    }

    pub(super) fn lower_terminator(&mut self, source_block: BlockId, term: &Terminator) {
        let mut diagnostics = Vec::new();
        let lowered = {
            let mut codegen = self.codegen();
            self.edges()
                .terminator(&mut codegen, source_block, term, &mut diagnostics)
        };
        self.record_fatals(diagnostics);
        if lowered {
            return;
        }
        match term {
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                let frame = self.poll_frame_address();
                let mut diagnostics = Vec::new();
                self.edges().dispatch_state(
                    &mut self.codegen(),
                    source_block,
                    frame,
                    cases,
                    (*default, default_args),
                    &mut diagnostics,
                );
                self.record_fatals(diagnostics);
            }
            Terminator::Return { values } => {
                self.emit_return(values.first().copied());
            }
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::Unreachable => unreachable!("the walker lowers {term:?}"),
        }
    }

    // ── Helpers ──

    /// Resolve a ValueId to its LLVM value.
    ///
    /// If the value was never defined, record a fatal diagnostic. The fallback
    /// value only keeps diagnostic collection moving; checked lowering refuses
    /// to expose the resulting function.
    #[cfg(test)]
    pub(super) fn resolve(&self, id: ValueId) -> BasicValueEnum<'ctx> {
        if let Some(val) = self.values.value(id) {
            val
        } else {
            self.record_fatal(format!(
                "ValueId %{} was used before being defined during LLVM lowering",
                id.0
            ));
            self.backend.context.i64_type().get_undef().into()
        }
    }

    /// Ensure a value is i64 (for NaN-boxed runtime calls).
    /// If it's already i64, return as-is. Otherwise, cast/extend.
    #[cfg(test)]
    pub(super) fn ensure_i64(&self, val: BasicValueEnum<'ctx>) -> inkwell::values::IntValue<'ctx> {
        crate::codegen_ssa::scalar::word_of(&mut self.codegen(), val)
    }

    #[cfg(test)]
    pub(super) fn unbox_ptr_bits(
        &self,
        bits: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        self.backend
            .builder
            .build_and(
                bits,
                i64_ty.const_int(nanbox::POINTER_MASK, false),
                "ptr_address",
            )
            .unwrap()
    }
}
