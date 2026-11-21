//! LLVM's placement of the shared plain-instance allocation and terminal free
//! (`molt_codegen_abi::object_lifecycle` owns the meaning). The allocation is
//! an always-inlined site body; the terminal free is one `noinline` body per
//! module that every release refcount site's slow call reaches.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use inkwell::values::IntValue;
use molt_codegen_abi::inline_refcount::RefcountStorage;
use molt_codegen_abi::list_item::ListLocking;
use molt_codegen_abi::object_lifecycle::{
    LifecycleWords, PlanSource, generate_builtin_type, generate_object_alloc,
    generate_release_fields, generate_release_terminal, generate_type_of,
};
use molt_codegen_abi::thread_record::ThreadRecordWord;
use molt_codegen_abi::word::WordBuilder;

impl<'ctx> LlvmBackend<'ctx> {
    /// The thread-record words the lifecycle generators read, in the function
    /// `builder` is positioned in.
    fn lifecycle_words(
        &self,
        builder: &inkwell::builder::Builder<'ctx>,
        profile_enabled: IntValue<'ctx>,
    ) -> LifecycleWords<IntValue<'ctx>> {
        LifecycleWords {
            profile_enabled,
            gc_record: self.load_thread_record_word(
                builder,
                ThreadRecordWord::GcRecord,
                "gc_record",
            ),
            missing_slot: self.load_thread_record_word(
                builder,
                ThreadRecordWord::MissingSlot,
                "missing_slot",
            ),
        }
    }

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
            let words = self.lifecycle_words(b.words.builder, profile_enabled);
            generate_object_alloc(
                b,
                class,
                plan,
                words,
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

    /// `type(value)`: `(context, value, admission) -> owned class`; its miss
    /// is `molt_type_of(value)`.
    pub(super) fn type_of_helper(&self) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(RuntimeFn::TypeOf);
        self.site_helper("__molt_type_of", 2, |b, words| {
            let &[value] = words else {
                unreachable!("the type_of helper's parameters")
            };
            generate_type_of(b, value, |b| {
                b.words
                    .builder
                    .build_execution_call(cold_fn, &[value.into()], "type_of_miss")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value()
            })
        })
    }

    /// `BUILTIN_TYPE(tag)`: `(context, tag, admission) -> owned class`; its
    /// miss is `molt_builtin_type(tag)`.
    pub(super) fn builtin_type_helper(&self) -> FunctionValue<'ctx> {
        let cold_fn = self.runtime_import(RuntimeFn::BuiltinType);
        self.site_helper("__molt_builtin_type", 2, |b, words| {
            let &[tag] = words else {
                unreachable!("the builtin_type helper's parameters")
            };
            let object_class = self.load_thread_record_word(
                b.words.builder,
                ThreadRecordWord::ObjectClass,
                "object_class",
            );
            generate_builtin_type(b, tag, object_class, |b| {
                b.words
                    .builder
                    .build_execution_call(cold_fn, &[tag.into()], "builtin_type_miss")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value()
            })
        })
    }

    /// The terminal release: `(context, bits, admission) -> None`, one
    /// `noinline` copy per module; its miss is `molt_dec_ref_obj`.
    pub(super) fn release_terminal_helper(&self) -> FunctionValue<'ctx> {
        let free = self.runtime_import(RuntimeFn::HeapFree);
        let cold_fn = self.runtime_import(RuntimeFn::DecRefObj);
        let release_fields = self.release_fields_helper();
        self.site_helper_placed("__molt_release_terminal", 2, "noinline", |b, words| {
            let &[bits] = words else {
                unreachable!("the terminal-release helper's parameters")
            };
            let profile_enabled = self.load_thread_record_word(
                b.words.builder,
                ThreadRecordWord::ProfileEnabled,
                "profile_enabled",
            );
            let words = self.lifecycle_words(b.words.builder, profile_enabled);
            let terminal_release = self.load_thread_record_word(
                b.words.builder,
                ThreadRecordWord::TerminalRelease,
                "terminal_release",
            );
            let admission = b.admission;
            generate_release_terminal(
                b,
                bits,
                admission,
                words,
                terminal_release,
                ListLocking::BUILD,
                RefcountStorage::for_build(molt_codegen_abi::MOLT_REFCOUNT_ATOMIC),
                |b, object| {
                    b.words
                        .builder
                        .build_execution_call(free, &[object.into()], "")
                        .unwrap();
                },
                |b, pointer, fields| {
                    let admission = b.admission;
                    b.words
                        .builder
                        .build_execution_call(
                            release_fields,
                            &[pointer.into(), fields.into(), admission.into()],
                            "",
                        )
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

    /// The field release: `(context, pointer, fields, admission) -> None`,
    /// one `noinline` copy per module, reached only from the terminal
    /// release; a last reference is `molt_dec_ref_obj`'s.
    pub(super) fn release_fields_helper(&self) -> FunctionValue<'ctx> {
        let dec_ref = self.runtime_import(RuntimeFn::DecRefObj);
        self.site_helper_placed("__molt_release_fields", 3, "noinline", |b, words| {
            let &[pointer, fields] = words else {
                unreachable!("the field-release helper's parameters")
            };
            generate_release_fields(b, pointer, fields, |b, child| {
                b.words
                    .builder
                    .build_execution_call(dec_ref, &[child.into()], "")
                    .unwrap();
            });
            b.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
        })
    }
}
