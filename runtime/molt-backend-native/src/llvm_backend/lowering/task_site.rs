//! LLVM's spelling of task construction. The protocol is
//! `molt_codegen_abi::task_site::generate_task_construction`; this module
//! only spells its runtime calls and runs it in an always-inlined helper, as
//! every shared generator runs (`site_helper`).

use super::frame_record::LlvmFrameBuilder;
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use inkwell::values::IntValue;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::task_site::{
    PayloadCustody, TaskConstructorLayout, TaskFrameContext, TaskPayload, TaskSiteBuilder,
    generate_task_construction,
};
use molt_ir::runtime_fn_generated::RuntimeFn;

/// The runtime entries a task helper calls.
#[derive(Clone, Copy)]
pub(super) struct LlvmTaskImports<'ctx> {
    task_new: FunctionValue<'ctx>,
    cancel_token_get_current: FunctionValue<'ctx>,
    task_register_execution: FunctionValue<'ctx>,
    asyncgen_new: FunctionValue<'ctx>,
    int_from_i64: FunctionValue<'ctx>,
}

fn custody_tag(custody: PayloadCustody) -> String {
    match custody {
        PayloadCustody::Borrowed => "b".into(),
        PayloadCustody::Unowned => "u".into(),
        PayloadCustody::RawInt => "r".into(),
        PayloadCustody::SameAs(first) => format!("s{first}"),
    }
}

/// A task helper's payload shape: what its words are, without the words.
#[derive(Clone, Debug, Eq, PartialEq)]
enum PayloadShape {
    Words(Vec<PayloadCustody>),
    BorrowedArguments { closure: bool, count: usize },
}

impl PayloadShape {
    fn of<W>(payload: &TaskPayload<'_, W>) -> Self {
        match payload {
            TaskPayload::Words(words) => Self::Words(words.iter().map(|&(_, c)| c).collect()),
            TaskPayload::BorrowedArguments { closure, count, .. } => Self::BorrowedArguments {
                closure: closure.is_some(),
                count: *count,
            },
        }
    }

    fn tag(&self) -> String {
        match self {
            Self::Words(custody) => custody
                .iter()
                .map(|&c| custody_tag(c))
                .collect::<Vec<_>>()
                .join("_"),
            Self::BorrowedArguments { closure, count } => {
                format!("args{}_{count}", if *closure { "_closure" } else { "" })
            }
        }
    }

    /// The helper words this payload passes.
    fn words(&self) -> usize {
        match self {
            Self::Words(custody) => custody.len(),
            Self::BorrowedArguments { closure, .. } => usize::from(*closure) + 1,
        }
    }
}

impl<'ctx> TaskSiteBuilder for LlvmFrameBuilder<'_, 'ctx> {
    fn task_new(
        &mut self,
        poll: Self::Word,
        frame_bytes: Self::Word,
        kind: Self::Word,
        function: Self::Word,
    ) -> Self::Word {
        let imports = self
            .task_imports
            .expect("a task helper sets its task imports");
        self.words
            .builder
            .build_execution_call(
                imports.task_new,
                &[
                    poll.into(),
                    frame_bytes.into(),
                    kind.into(),
                    function.into(),
                ],
                "task_new",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    fn cancel_token_get_current(&mut self) -> Self::Word {
        let imports = self
            .task_imports
            .expect("a task helper sets its task imports");
        self.words
            .builder
            .build_execution_call(imports.cancel_token_get_current, &[], "task_current_token")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    fn task_register_execution(&mut self, task: Self::Word, token: Self::Word) {
        let imports = self
            .task_imports
            .expect("a task helper sets its task imports");
        let none = self
            .words
            .context
            .i64_type()
            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        self.words
            .builder
            .build_execution_call(
                imports.task_register_execution,
                &[task.into(), token.into(), none.into()],
                "",
            )
            .unwrap();
    }

    fn asyncgen_new(&mut self, generator: Self::Word) -> Self::Word {
        let imports = self
            .task_imports
            .expect("a task helper sets its task imports");
        self.words
            .builder
            .build_execution_call(imports.asyncgen_new, &[generator.into()], "task_asyncgen")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    fn int_from_i64(&mut self, raw: Self::Word) -> Self::Word {
        let imports = self
            .task_imports
            .expect("a task helper sets its task imports");
        self.words
            .builder
            .build_execution_call(imports.int_from_i64, &[raw.into()], "task_payload_int")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }
}

impl<'ctx> LlvmBackend<'ctx> {
    fn task_imports(&self) -> LlvmTaskImports<'ctx> {
        LlvmTaskImports {
            task_new: self.runtime_import(RuntimeFn::TaskNew),
            cancel_token_get_current: self.runtime_import(RuntimeFn::CancelTokenGetCurrent),
            task_register_execution: self.runtime_import(RuntimeFn::TaskRegisterExecution),
            asyncgen_new: self.runtime_import(RuntimeFn::AsyncgenNew),
            int_from_i64: self.runtime_import(RuntimeFn::IntFromI64),
        }
    }

    /// The always-inlined helper constructing a `layout` task: its words are
    /// `[poll, frame bytes, context word (none for NoFrame), payload words,
    /// RC admission]`, the payload words being each slot's word or a
    /// trampoline's closure (if any) and argument array. One helper per
    /// layout, context source and payload shape.
    fn task_site_helper(
        &self,
        layout: TaskConstructorLayout,
        source: TaskFrameContext<()>,
        shape: PayloadShape,
    ) -> FunctionValue<'ctx> {
        let name = format!(
            "__molt_task_site_{}_{}_{}",
            layout.diagnostic_name().replace(' ', "_"),
            match source {
                TaskFrameContext::Function(()) => "fn",
                TaskFrameContext::PendingInvocation { .. } => "pending",
                TaskFrameContext::NoFrame => "noframe",
            },
            shape.tag(),
        );
        let context_words = usize::from(source != TaskFrameContext::NoFrame);
        let params = 2 + context_words + shape.words() + 1;
        let imports = self.task_imports();
        self.site_helper(&name, params, move |b, words| {
            b.task_imports = Some(imports);
            let (poll, frame_bytes) = (words[0], words[1]);
            let context = source.map(|()| words[2]);
            let payload_words = &words[2 + context_words..];
            let held: Vec<(IntValue<'ctx>, PayloadCustody)>;
            let payload = match &shape {
                PayloadShape::Words(custody) => {
                    held = payload_words
                        .iter()
                        .copied()
                        .zip(custody.iter().copied())
                        .collect();
                    TaskPayload::Words(&held)
                }
                PayloadShape::BorrowedArguments { closure, count } => {
                    TaskPayload::BorrowedArguments {
                        closure: closure.then(|| payload_words[0]),
                        arguments: payload_words[usize::from(*closure)],
                        count: *count,
                    }
                }
            };
            generate_task_construction(
                b,
                layout,
                crate::GENERATOR_CONTROL_BYTES,
                poll,
                frame_bytes,
                context,
                payload,
            )
        })
    }

    /// Call the task helper at `builder`'s position, in whichever function it
    /// builds (this function's body or a task trampoline).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_task_site(
        &self,
        builder: &inkwell::builder::Builder<'ctx>,
        admission: IntValue<'ctx>,
        layout: TaskConstructorLayout,
        poll: IntValue<'ctx>,
        frame_bytes: IntValue<'ctx>,
        context: TaskFrameContext<IntValue<'ctx>>,
        payload: TaskPayload<'_, IntValue<'ctx>>,
        name: &str,
    ) -> IntValue<'ctx> {
        let mut arguments: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![poll.into(), frame_bytes.into()];
        let source = context.map(|word| arguments.push(word.into()));
        let helper = self.task_site_helper(layout, source, PayloadShape::of(&payload));
        match payload {
            TaskPayload::Words(words) => {
                for &(word, _) in words {
                    arguments.push(word.into());
                }
            }
            TaskPayload::BorrowedArguments {
                closure,
                arguments: array,
                ..
            } => {
                if let Some(closure) = closure {
                    arguments.push(closure.into());
                }
                arguments.push(array.into());
            }
        }
        arguments.push(admission.into());
        builder
            .build_execution_call(helper, &arguments, name)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }
}

impl<'ctx> crate::codegen_ssa::task::TaskPlacement for LlvmCodegen<'_, 'ctx> {
    fn poll_frame_bytes(&mut self, symbol: &str) -> Self::Word {
        self.words
            .builder
            .build_load(
                self.words.context.i64_type(),
                self.backend.stateful_frame_size(symbol).as_pointer_value(),
                "task_frame_size",
            )
            .unwrap()
            .into_int_value()
    }
    fn place_task(
        &mut self,
        layout: TaskConstructorLayout,
        poll: Self::Word,
        frame_bytes: Self::Word,
        context: TaskFrameContext<Self::Word>,
        payload: TaskPayload<'_, Self::Word>,
        name: &str,
    ) -> Self::Word {
        let admission = self
            .llvm_refcount_site(RefcountOperation::Release)
            .admission;
        self.backend.call_task_site(
            self.words.builder,
            admission,
            layout,
            poll,
            frame_bytes,
            context,
            payload,
            name,
        )
    }
}
