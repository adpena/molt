//! LLVM instruction selection for the shared list item read and store
//! (`molt_codegen_abi::list_item` owns the meaning). As the method site
//! bodies are, each runtime miss entry gets one always-inlined internal body,
//! so the access's explicit blocks never split the caller's TIR block.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use inkwell::values::IntValue;
use molt_codegen_abi::list_item::{ListLocking, generate_list_item_get, generate_list_item_set};
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> FunctionLowering<'ctx, '_> {
    /// The read body whose miss calls `cold(list, index)`: `(context, list,
    /// index, admission) -> owned item`.
    pub(super) fn ensure_list_item_get_helper(&self, cold: RuntimeFn) -> FunctionValue<'ctx> {
        let cold_fn = self.ensure_runtime_import(cold);
        self.site_helper(
            &format!("__molt_list_item_get_{}", cold.symbol()),
            3,
            false,
            |b, words| {
                let &[list, index] = words else {
                    unreachable!("the list read helper's parameters")
                };
                generate_list_item_get(b, list, index, ListLocking::BUILD, |b| {
                    b.words
                        .builder
                        .build_execution_call(
                            cold_fn,
                            &[list.into(), index.into()],
                            "list_get_miss",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                        .into_int_value()
                })
            },
        )
    }

    /// The store body whose miss calls `molt_setitem_method(list, index,
    /// value)`: `(context, list, index, value, admission) -> None or the
    /// runtime's word`.
    pub(super) fn ensure_list_item_set_helper(&self) -> FunctionValue<'ctx> {
        let cold_fn = self.ensure_runtime_import(RuntimeFn::SetitemMethod);
        self.site_helper("__molt_list_item_set", 4, false, |b, words| {
            let &[list, index, value] = words else {
                unreachable!("the list store helper's parameters")
            };
            let none = b.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE);
            generate_list_item_set(b, list, index, value, ListLocking::BUILD, none, |b| {
                b.words
                    .builder
                    .build_execution_call(
                        cold_fn,
                        &[list.into(), index.into(), value.into()],
                        "list_set_miss",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value()
            })
        })
    }
}

impl<'ctx> FunctionLowering<'ctx, '_> {
    /// Whether `container`'s refined TIR type admits a list: a dict, tuple,
    /// set or str keeps its runtime entry without the list tests.
    pub(super) fn container_may_be_list(&self, container: ValueId) -> bool {
        !matches!(
            self.func.value_types.get(&container),
            Some(TirType::Dict(_, _) | TirType::Tuple(_) | TirType::Set(_) | TirType::Str)
        )
    }

    /// The RC admission word a list item body passes to its retain and
    /// release (`molt_codegen_abi::list_item`).
    pub(super) fn list_item_admission(&self) -> IntValue<'ctx> {
        self.ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Retain)
            .admission
    }
}
