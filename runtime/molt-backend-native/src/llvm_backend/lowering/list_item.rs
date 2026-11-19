//! LLVM's placement of the shared list item read and store
//! (`molt_codegen_abi::list_item` owns the meaning; the walker's object
//! family calls it as a site body). As the method site bodies are, each
//! runtime miss entry gets one always-inlined internal body.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_codegen_abi::list_item::{
    ItemContainers, ListLocking, generate_list_item_get, generate_list_item_set,
};
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> LlvmBackend<'ctx> {
    /// The closure slot read (`molt_codegen_abi::closure_slot`), always
    /// inlined: `(context, owner, offset, admission) -> owned word`.
    pub(super) fn closure_load_helper(&self) -> FunctionValue<'ctx> {
        self.site_helper("__molt_closure_load", 3, |b, words| {
            let &[owner, offset] = words else {
                unreachable!("the closure load helper's parameters")
            };
            molt_codegen_abi::closure_slot::generate_closure_load(b, owner, offset)
        })
    }

    /// The closure slot store, whose miss calls `molt_closure_store(owner,
    /// offset, value)`: `(context, owner, offset, value, admission) -> None`.
    pub(super) fn closure_store_helper(&self) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(RuntimeFn::ClosureStore);
        self.site_helper("__molt_closure_store", 4, |b, words| {
            let &[owner, offset, value] = words else {
                unreachable!("the closure store helper's parameters")
            };
            molt_codegen_abi::closure_slot::generate_closure_store(
                b,
                owner,
                offset,
                value,
                ListLocking::BUILD,
                |b| {
                    b.words
                        .builder
                        .build_execution_call(
                            cold_fn,
                            &[owner.into(), offset.into(), value.into()],
                            "closure_store_miss",
                        )
                        .unwrap();
                },
            );
            b.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
        })
    }

    /// The read body over the `containers` a site's proof admits, whose miss
    /// calls `cold(sequence, index)`: `(context, sequence, index, admission)
    /// -> owned item`.
    pub(super) fn list_item_get_helper(
        &self,
        containers: ItemContainers,
        cold: RuntimeFn,
    ) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(cold);
        let kinds = match containers {
            ItemContainers::Lists => "lists",
            ItemContainers::Tuple => "tuple",
            ItemContainers::ListsOrTuple => "sequence",
        };
        self.site_helper(
            &format!("__molt_item_get_{kinds}_{}", cold.symbol()),
            3,
            |b, words| {
                let &[sequence, index] = words else {
                    unreachable!("the item read helper's parameters")
                };
                generate_list_item_get(b, sequence, index, containers, ListLocking::BUILD, |b| {
                    b.words
                        .builder
                        .build_execution_call(
                            cold_fn,
                            &[sequence.into(), index.into()],
                            "item_get_miss",
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
    pub(super) fn list_item_set_helper(&self) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(RuntimeFn::SetitemMethod);
        self.site_helper("__molt_list_item_set", 4, |b, words| {
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
