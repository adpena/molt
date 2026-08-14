use crate::OpIR;
use crate::native_callable_abi::{NativeCallableLowering, parse_native_callable_abi};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm::task_runtime::WasmTaskRuntimeLayout;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use molt_tir::tir::op_kinds_generated::simpleir_kind_is_wasm_stateful_dispatch;
use wasm_encoder::ValType;

#[derive(Default)]
pub(super) struct FrameRuntimeRequirements {
    needs_field_fast: bool,
    needs_native_forward_f32: bool,
    needs_alloc_resolve: bool,
    arena_local: Option<u32>,
    has_arena_eligible: bool,
    stateful: bool,
    saw_jump_or_label: bool,
    fast_int_count: usize,
}

impl FrameRuntimeRequirements {
    pub(super) fn observe_op(&mut self, scalar_plan: &ScalarRepresentationPlan, op: &OpIR) {
        if wasm_scalar_integer_fast_path_for_op(scalar_plan, op) {
            self.fast_int_count += 1;
        }
        if op.arena_eligible == Some(true) {
            self.has_arena_eligible = true;
        }
        match op.kind.as_str() {
            "store" | "store_init" | "load" | "guarded_load" | "guarded_field_get"
            | "guarded_field_set" | "guarded_field_init" => self.needs_field_fast = true,
            "invoke_ffi"
                if op
                    .native_callable_abi
                    .as_deref()
                    .and_then(parse_native_callable_abi)
                    .is_some_and(|abi| abi.lowering() == NativeCallableLowering::ForwardF32) =>
            {
                self.needs_native_forward_f32 = true;
            }
            kind if simpleir_kind_is_wasm_stateful_dispatch(kind) => self.stateful = true,
            "jump" | "label" => self.saw_jump_or_label = true,
            "alloc_task" => {
                let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
                let layout = WasmTaskRuntimeLayout::for_alloc_task_kind(op.task_kind.as_deref());
                if layout.needs_alloc_resolve(has_args) {
                    self.needs_alloc_resolve = true;
                }
            }
            "call_async" => {
                let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
                if WasmTaskRuntimeLayout::for_call_async().needs_alloc_resolve(has_args) {
                    self.needs_alloc_resolve = true;
                }
            }
            _ => {}
        }
    }

    pub(super) fn ensure_synthetic_locals(
        &mut self,
        locals: &mut WasmFrameLocals,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) {
        if self.needs_field_fast || self.needs_native_forward_f32 {
            locals.ensure_synthetic(WasmFrameSyntheticLocal::WasmTmp0, local_types, local_count);
        }

        if self.needs_field_fast {
            locals.ensure_synthetic(WasmFrameSyntheticLocal::WasmTmp1, local_types, local_count);
        }

        if self.needs_alloc_resolve {
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmAllocResolve,
                local_types,
                local_count,
            );
        }

        if self.has_arena_eligible {
            self.arena_local = Some(locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmScopeArena,
                local_types,
                local_count,
            ));
        }
    }

    pub(super) fn stateful(&self) -> bool {
        self.stateful
    }

    pub(super) fn jumpful(&self) -> bool {
        !self.stateful && self.saw_jump_or_label
    }

    pub(super) fn tail_call_eligible(&self) -> bool {
        !self.stateful
    }

    pub(super) fn fast_int_count(&self) -> usize {
        self.fast_int_count
    }

    pub(super) fn arena_local(&self) -> Option<u32> {
        self.arena_local
    }
}
