//! LLVM instruction selection for the shared proven field read and store
//! (`molt_codegen_abi::field_slot` owns the meaning). Each field offset gets
//! one always-inlined internal body per direction, as the list item bodies
//! do, so the access's explicit blocks never split the caller's TIR block.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_codegen_abi::field_slot::{generate_field_slot_get, generate_field_slot_set};
use molt_codegen_abi::list_item::ListLocking;
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> FunctionLowering<'ctx, '_> {
    /// The field read body at `offset`: `(context, object, admission) ->
    /// owned value`; its miss calls `molt_object_field_get_ptr(pointer,
    /// offset)`.
    fn ensure_field_slot_get_helper(&self, offset: i32) -> FunctionValue<'ctx> {
        let cold_fn = self.ensure_runtime_import(RuntimeFn::ObjectFieldGetPtr);
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
    fn ensure_field_slot_set_helper(&self, offset: i32) -> FunctionValue<'ctx> {
        let cold_fn = self.ensure_runtime_import(RuntimeFn::ObjectFieldSetPtr);
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

    /// A proven field's byte offset (the op's `value` attribute).
    fn field_slot_offset(op: &TirOp) -> i32 {
        match op.attrs.get("value") {
            Some(AttrValue::Int(offset)) => {
                i32::try_from(*offset).expect("a proven field offset fits an i32")
            }
            _ => panic!("a proven field access carries its offset"),
        }
    }

    /// `load`: a proven field read, through the shared generator.
    pub(super) fn emit_field_slot_get(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let get_fn = self.ensure_field_slot_get_helper(Self::field_slot_offset(op));
        let args = [
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Word(self.list_item_admission().into()),
        ];
        let value =
            self.borrowed_runtime_call_value(get_fn, &args, false, "field_load", "field_get");
        self.values.set_value(result_id, value.into());
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    /// `store`: a proven field store, through the shared generator.
    pub(super) fn emit_field_slot_set(&mut self, op: &TirOp) {
        let set_fn = self.ensure_field_slot_set_helper(Self::field_slot_offset(op));
        let args = [
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
            RuntimeArg::Word(self.list_item_admission().into()),
        ];
        self.emit_borrowed_runtime_call(
            op,
            set_fn,
            &args,
            ResultCustody::Unowned,
            "field_store",
            "field_set",
        );
    }
}
