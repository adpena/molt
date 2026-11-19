use super::*;
use crate::codegen_ssa::custody;

/// A runtime-call argument on LLVM: the walker's, over LLVM values.
pub(super) type RuntimeArg<'ctx> = custody::RuntimeArg<BasicValueEnum<'ctx>>;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Match native/WASM invocation ownership: entry can fail before a frame
    /// exists, and every return (including suspension) closes only its own.
    pub(super) fn initialize_owned_execution_frame(
        &mut self,
        layout: crate::codegen_ssa::frame::FrameLayout,
    ) {
        use crate::codegen_ssa::frame::{FrameEntry, RecordPlacement};
        assert!(self.owned_frame_entered.is_none());
        self.owned_frame_entered = crate::codegen_ssa::frame::initialize_owned_execution_frame(
            &mut self.codegen(),
            self.func.execution_context,
            || layout.entry.kind(),
        );
        self.frame_homes = match (layout.homes, layout.entry) {
            (None, _) => None,
            (Some(_), FrameEntry::Absent) => {
                Some(self.build_entry_i64_alloca("borrowed_frame_homes"))
            }
            (Some(_), _) => self.owned_frame_entered.map(|frame| frame.slot),
        };
        let homes = self.frame_homes;
        self.codegen().set_frame_homes(homes);
    }

    pub(super) fn program_slot_id(
        &self,
        kind: molt_ir::code_slots::ProgramSlotKind,
        op: &TirOp,
    ) -> inkwell::values::IntValue<'ctx> {
        let owner = match op.attrs.get("s_value") {
            Some(AttrValue::Str(owner)) => owner.as_str(),
            _ => panic!("admitted program-slot owner"),
        };
        self.load_program_slot(kind, owner)
    }

    fn load_program_slot(
        &self,
        kind: molt_ir::code_slots::ProgramSlotKind,
        owner: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        self.codegen().program_slot(kind, owner)
    }

    /// Emit a borrowed runtime call and bind or retire its word by `custody`.
    pub(super) fn emit_borrowed_runtime_call(
        &mut self,
        op: &TirOp,
        callee: FunctionValue<'ctx>,
        args: &[RuntimeArg<'ctx>],
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let (mut cx, mut b) = self.walker();
        let callee = crate::codegen_ssa::custody::CallTarget::Callee(callee);
        cx.runtime_call(&mut b, op, callee, args, custody, label, call_name);
    }

    /// `emit_borrowed_runtime_call` over every operand of `op`, in order.
    pub(super) fn emit_positional_runtime_call(
        &mut self,
        op: &TirOp,
        callee: FunctionValue<'ctx>,
        custody: ResultCustody,
        label: &str,
        call_name: &str,
    ) {
        let (mut cx, mut b) = self.walker();
        cx.positional_runtime_call(&mut b, op, callee, custody, label, call_name);
    }

    // ── Representation authority ──

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
        cx.walker_owned_op(&mut codegen, &at, op)
    }

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

    /// The machine-level type of a lowered value: `I64`, `F64` or `Bool` when
    /// it is held raw, `DynBox` when it is a boxed word. It names a
    /// representation for lowering helpers, never a proven Python type; that
    /// is `TirFunction::value_types`.
    pub(super) fn repr_ty(&self, id: ValueId) -> TirType {
        self.values.repr(id).map_or(
            TirType::DynBox,
            crate::codegen_ssa::HeldCarrier::machine_type,
        )
    }

    /// Resolve a ValueId to its LLVM value.
    ///
    /// If the value was never defined, record a fatal diagnostic. The fallback
    /// value only keeps diagnostic collection moving; checked lowering refuses
    /// to expose the resulting function.
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
    pub(super) fn ensure_i64(&self, val: BasicValueEnum<'ctx>) -> inkwell::values::IntValue<'ctx> {
        crate::codegen_ssa::scalar::word_of(&mut self.codegen(), val)
    }

    /// The walker's async-work poll (`FunctionCx::async_work_poll`).
    pub(super) fn emit_async_work_poll(
        &mut self,
        payload: bool,
        cold_location: Option<i64>,
    ) -> BasicValueEnum<'ctx> {
        let (mut cx, mut b) = self.walker();
        cx.async_work_poll(&mut b, payload, cold_location).into()
    }

    /// The module's declaration of runtime entry `function` (HF-821): its
    /// linker symbol, type and attributes come from the generated record.
    pub(super) fn ensure_runtime_import(&self, function: RuntimeFn) -> FunctionValue<'ctx> {
        self.backend.runtime_import(function)
    }

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
