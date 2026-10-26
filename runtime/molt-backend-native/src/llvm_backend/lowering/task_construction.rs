//! The allocation/failure/completion protocol shared by direct task creation
//! and Python-callable task trampolines. Payload acquisition stays in its
//! owning caller: SSA values may need boxing; trampoline words are borrowed.

use super::*;
use crate::runtime_import_abi::{
    MOLT_ASYNCGEN_NEW, MOLT_CANCEL_TOKEN_GET_CURRENT, MOLT_TASK_NEW, MOLT_TASK_REGISTER_EXECUTION,
};
use inkwell::builder::Builder;
use inkwell::values::{IntValue, PointerValue};
use molt_tir::trampolines::{TaskCompletion, TaskConstructorLayout};

enum CompletionImports<'ctx> {
    ReturnTask,
    Register {
        token: FunctionValue<'ctx>,
        register: FunctionValue<'ctx>,
    },
    Wrap {
        wrap: FunctionValue<'ctx>,
        release: FunctionValue<'ctx>,
    },
}

pub(super) struct TaskConstructorImports<'ctx> {
    allocate: FunctionValue<'ctx>,
    kind: i64,
    completion: CompletionImports<'ctx>,
}

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Project the canonical runtime signatures once for the selected task
    /// completion. No second declaration table or permissive import path.
    pub(super) fn task_constructor_imports(
        &self,
        layout: TaskConstructorLayout,
    ) -> TaskConstructorImports<'ctx> {
        let completion = match layout.completion() {
            TaskCompletion::ReturnTask => CompletionImports::ReturnTask,
            TaskCompletion::RegisterCancelToken => CompletionImports::Register {
                token: self.ensure_runtime_import(MOLT_CANCEL_TOKEN_GET_CURRENT),
                register: self.ensure_runtime_import(MOLT_TASK_REGISTER_EXECUTION),
            },
            TaskCompletion::WrapAsyncGen => CompletionImports::Wrap {
                wrap: self.ensure_runtime_import(MOLT_ASYNCGEN_NEW),
                release: self.ensure_runtime_import(MOLT_DEC_REF_OBJ),
            },
        };
        TaskConstructorImports {
            allocate: self.ensure_runtime_import(MOLT_TASK_NEW),
            kind: crate::native_task_runtime_kind_bits(layout.runtime_kind()),
            completion,
        }
    }
}

pub(super) struct TaskConstruction<'ctx> {
    pub object: IntValue<'ctx>,
    pub payload: PointerValue<'ctx>,
    pub allocation: BasicBlock<'ctx>,
    pub initialized: BasicBlock<'ctx>,
    pub continuation: BasicBlock<'ctx>,
    completion: CompletionImports<'ctx>,
}

impl<'ctx> TaskConstruction<'ctx> {
    pub(super) fn begin(
        backend: &LlvmBackend<'ctx>,
        builder: &Builder<'ctx>,
        function: FunctionValue<'ctx>,
        poll: IntValue<'ctx>,
        imports: TaskConstructorImports<'ctx>,
        closure_size: i64,
        names: (&str, &str, &str),
    ) -> Self {
        let i64_ty = backend.context.i64_type();
        let object = builder
            .build_call(
                imports.allocate,
                &[
                    poll.into(),
                    i64_ty.const_int(closure_size as u64, false).into(),
                    i64_ty.const_int(imports.kind as u64, false).into(),
                ],
                names.0,
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let allocation = builder.get_insert_block().unwrap();
        let initialized = backend.context.append_basic_block(function, names.1);
        let continuation = backend.context.append_basic_block(function, names.2);
        let allocated = builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                object,
                i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false),
                "task_allocated",
            )
            .unwrap();
        builder
            .build_conditional_branch(allocated, initialized, continuation)
            .unwrap();
        builder.position_at_end(initialized);
        let address = builder
            .build_and(
                object,
                i64_ty.const_int(nanbox::POINTER_MASK, false),
                "task_address",
            )
            .unwrap();
        let payload = builder
            .build_int_to_ptr(
                address,
                backend.context.ptr_type(inkwell::AddressSpace::default()),
                "task_obj_ptr",
            )
            .unwrap();
        Self {
            object,
            payload,
            allocation,
            initialized,
            continuation,
            completion: imports.completion,
        }
    }

    pub(super) fn finish(
        self,
        backend: &LlvmBackend<'ctx>,
        builder: &Builder<'ctx>,
    ) -> IntValue<'ctx> {
        let wrapped = matches!(self.completion, CompletionImports::Wrap { .. });
        let result = match self.completion {
            CompletionImports::ReturnTask => self.object,
            CompletionImports::Register { token, register } => {
                let token = builder
                    .build_call(token, &[], "task_current_token")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                builder
                    .build_call(
                        register,
                        &[
                            self.object.into(),
                            token.into(),
                            backend
                                .context
                                .i64_type()
                                .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                                .into(),
                        ],
                        "",
                    )
                    .unwrap();
                self.object
            }
            CompletionImports::Wrap { wrap, release } => {
                let wrapped = builder
                    .build_call(wrap, &[self.object.into()], "task_asyncgen")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                builder
                    .build_call(release, &[self.object.into()], "")
                    .unwrap();
                wrapped
            }
        };
        let initialized_end = builder.get_insert_block().unwrap();
        builder
            .build_unconditional_branch(self.continuation)
            .unwrap();
        builder.position_at_end(self.continuation);
        if wrapped {
            let phi = builder
                .build_phi(backend.context.i64_type(), "task_result")
                .unwrap();
            phi.add_incoming(&[(&self.object, self.allocation), (&result, initialized_end)]);
            phi.as_basic_value().into_int_value()
        } else {
            self.object
        }
    }
}
