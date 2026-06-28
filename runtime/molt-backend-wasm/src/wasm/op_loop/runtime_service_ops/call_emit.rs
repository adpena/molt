use super::super::result_sink::{store_non_none_result_or_drop, store_result_or_drop};
use super::RuntimeServiceOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy)]
pub(super) enum RuntimeServiceArg {
    Local(usize),
    OpValueI64(&'static str),
}

#[derive(Clone, Copy)]
pub(super) enum RuntimeServiceSink {
    ResultOrDrop,
    NonNoneResultOrDrop,
    Drop,
    None,
}

#[derive(Clone, Copy)]
pub(super) struct RuntimeServiceCall<'a> {
    import: &'a str,
    args: &'a [RuntimeServiceArg],
    sink: RuntimeServiceSink,
}

impl<'a> RuntimeServiceCall<'a> {
    pub(super) const fn result(import: &'a str, args: &'a [RuntimeServiceArg]) -> Self {
        Self {
            import,
            args,
            sink: RuntimeServiceSink::ResultOrDrop,
        }
    }

    pub(super) const fn non_none(import: &'a str, args: &'a [RuntimeServiceArg]) -> Self {
        Self {
            import,
            args,
            sink: RuntimeServiceSink::NonNoneResultOrDrop,
        }
    }

    pub(super) const fn drop(import: &'a str, args: &'a [RuntimeServiceArg]) -> Self {
        Self {
            import,
            args,
            sink: RuntimeServiceSink::Drop,
        }
    }

    pub(super) const fn no_result(import: &'a str, args: &'a [RuntimeServiceArg]) -> Self {
        Self {
            import,
            args,
            sink: RuntimeServiceSink::None,
        }
    }
}

pub(super) fn emit_runtime_service_call(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    call: RuntimeServiceCall<'_>,
) {
    for arg in call.args {
        match *arg {
            RuntimeServiceArg::Local(index) => {
                let args = op
                    .args
                    .as_ref()
                    .unwrap_or_else(|| panic!("{} missing runtime-service args", op.kind));
                let name = args.get(index).unwrap_or_else(|| {
                    panic!(
                        "{} missing runtime-service arg {index}; only {} args present",
                        op.kind,
                        args.len()
                    )
                });
                func.instruction(&Instruction::LocalGet(context.locals[name]));
            }
            RuntimeServiceArg::OpValueI64(message) => {
                func.instruction(&Instruction::I64Const(op.value.expect(message)));
            }
        }
    }

    emit_call(func, context.reloc_enabled, context.import_ids[call.import]);
    match call.sink {
        RuntimeServiceSink::ResultOrDrop => store_result_or_drop(func, op, context.locals),
        RuntimeServiceSink::NonNoneResultOrDrop => {
            store_non_none_result_or_drop(func, op, context.locals)
        }
        RuntimeServiceSink::Drop => {
            func.instruction(&Instruction::Drop);
        }
        RuntimeServiceSink::None => {}
    }
}
