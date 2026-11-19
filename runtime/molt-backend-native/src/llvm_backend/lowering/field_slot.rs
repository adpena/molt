//! LLVM's placement of the shared proven field read and store
//! (`molt_codegen_abi::field_slot` owns the meaning; the walker's object
//! family calls it as a site body). Each field offset gets one
//! always-inlined internal body per direction, as the list item bodies do.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_codegen_abi::field_slot::{generate_field_slot_get, generate_field_slot_set};
use molt_codegen_abi::list_item::ListLocking;
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> LlvmBackend<'ctx> {
    /// The field read body at `offset`: `(context, object, admission) ->
    /// owned value`; its miss calls `molt_object_field_get_ptr(pointer,
    /// offset)`.
    pub(super) fn field_slot_get_helper(&self, offset: i32) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(RuntimeFn::ObjectFieldGetPtr);
        self.site_helper(&format!("__molt_field_slot_get_{offset}"), 2, |b, words| {
            let &[object] = words else {
                unreachable!("the field read helper's parameters")
            };
            generate_field_slot_get(b, object, offset, ListLocking::BUILD, |b, pointer| {
                let offset = b.constant(offset as i64 as u64);
                b.words
                    .builder
                    .build_execution_call(
                        cold_fn,
                        &[pointer.into(), offset.into()],
                        "field_get_miss",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value()
            })
        })
    }

    /// The field store body at `offset`: `(context, object, value,
    /// admission) -> None`; its miss calls `molt_object_field_set_ptr(pointer,
    /// offset, value)`.
    pub(super) fn field_slot_set_helper(&self, offset: i32) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(RuntimeFn::ObjectFieldSetPtr);
        self.site_helper(&format!("__molt_field_slot_set_{offset}"), 3, |b, words| {
            let &[object, value] = words else {
                unreachable!("the field store helper's parameters")
            };
            generate_field_slot_set(
                b,
                object,
                offset,
                value,
                ListLocking::BUILD,
                |b, pointer| {
                    let offset = b.constant(offset as i64 as u64);
                    b.words
                        .builder
                        .build_execution_call(
                            cold_fn,
                            &[pointer.into(), offset.into(), value.into()],
                            "field_set_miss",
                        )
                        .unwrap();
                },
            );
            b.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
        })
    }
}
