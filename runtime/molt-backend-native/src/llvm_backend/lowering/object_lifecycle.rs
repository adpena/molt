//! LLVM's placement of the shared plain-instance allocation and terminal free
//! (`molt_codegen_abi::object_lifecycle` owns the meaning). The allocation is
//! an always-inlined site body; the terminal free is one `noinline` body per
//! module that every release refcount site's slow call reaches.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_codegen_abi::list_item::ListLocking;
use molt_codegen_abi::object_lifecycle::{
    LifecycleWords, PlanSource, generate_object_alloc, generate_release_terminal,
};
use molt_codegen_abi::thread_record::ThreadRecordWord;
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> LlvmBackend<'ctx> {
    /// The allocation body: `(context, class, profile, admission) -> owned
    /// object`; its miss is `molt_object_new` (`root`) or
    /// `molt_object_new_bound(class)`.
    pub(super) fn object_alloc_helper(&self, root: bool, plan: PlanSource) -> FunctionValue<'ctx> {
        let allocate = self.runtime_import(RuntimeFn::HeapAlloc);
        let cold_fn = self.runtime_import(if root {
            RuntimeFn::ObjectNew
        } else {
            RuntimeFn::ObjectNewBound
        });
        let name = match (root, plan) {
            (true, PlanSource::Guarded) => "__molt_object_alloc_root".to_owned(),
            (false, PlanSource::Guarded) => "__molt_object_alloc".to_owned(),
            (root, PlanSource::Proven(plan)) => {
                format!(
                    "__molt_object_alloc_{}_{plan:x}",
                    if root { "root" } else { "class" }
                )
            }
        };
        self.site_helper(&name, 3, |b, words| {
            let &[class, profile_enabled] = words else {
                unreachable!("the allocation helper's parameters")
            };
            generate_object_alloc(
                b,
                class,
                plan,
                LifecycleWords { profile_enabled },
                ListLocking::BUILD,
                |b, bytes| {
                    b.words
                        .builder
                        .build_execution_call(allocate, &[bytes.into()], "heap_alloc")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                        .into_int_value()
                },
                |b| {
                    let args: Vec<_> = if root { vec![] } else { vec![class.into()] };
                    b.words
                        .builder
                        .build_execution_call(cold_fn, &args, "object_new_miss")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                        .into_int_value()
                },
            )
        })
    }

    /// The terminal release: `(context, bits, admission) -> None`, one
    /// `noinline` copy per module; its miss is `molt_dec_ref_obj`.
    pub(super) fn release_terminal_helper(&self) -> FunctionValue<'ctx> {
        let free = self.runtime_import(RuntimeFn::HeapFree);
        let cold_fn = self.runtime_import(RuntimeFn::DecRefObj);
        self.site_helper_placed("__molt_release_terminal", 2, "noinline", |b, words| {
            let &[bits] = words else {
                unreachable!("the terminal-release helper's parameters")
            };
            let profile_enabled = self.load_thread_record_word(
                b.words.builder,
                ThreadRecordWord::ProfileEnabled,
                "profile_enabled",
            );
            let terminal_release = self.load_thread_record_word(
                b.words.builder,
                ThreadRecordWord::TerminalRelease,
                "terminal_release",
            );
            generate_release_terminal(
                b,
                bits,
                LifecycleWords { profile_enabled },
                terminal_release,
                ListLocking::BUILD,
                |b, base, bytes| {
                    b.words
                        .builder
                        .build_execution_call(free, &[base.into(), bytes.into()], "")
                        .unwrap();
                },
                |b| {
                    b.words
                        .builder
                        .build_execution_call(cold_fn, &[bits.into()], "")
                        .unwrap();
                },
            );
            b.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
        })
    }
}
