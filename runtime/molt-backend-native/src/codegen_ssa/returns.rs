//! Function return and execution-frame custody, including the empty early
//! exit used when the caller will observe a pending exception.
use super::builder::CodegenBuilder;
use super::control::coerce;
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use molt_codegen_abi::frame_record::FrameEntryKind;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::tir::values::ValueId;

/// Local custody is retired before popping any homes. A zero base means this
/// invocation failed before linking the frame.
#[derive(Clone, Copy)]
pub(crate) struct OwnedFrame<S> {
    pub(crate) slot: S,
    pub(crate) kind: Option<FrameEntryKind>,
}

pub(crate) fn emit_return<B: CodegenBuilder>(
    cx: &mut FunctionCx<'_, B>,
    b: &mut B,
    abi: &NativeFunctionLinkageAbi,
    value: Option<ValueId>,
    frame: Option<&OwnedFrame<B::Slot>>,
) {
    let result = match (&abi.return_type, value) {
        (None, None) => None,
        (None, Some(_)) => {
            cx.fatal("returns a value through a native-void linkage ABI");
            b.unreachable();
            return;
        }
        (Some(target), id) => {
            let (word, source) = match id {
                Some(id) => {
                    let operands = cx.operands();
                    (operands.resolve(b, id), operands.held(id))
                }
                None => {
                    let none = b.constant(molt_codegen_abi::QNAN_TAG_NONE_I64 as u64);
                    (b.word_value(none), HeldCarrier::Boxed)
                }
            };
            let Some(coerced) = coerce(b, cx.plan, word, source, id, target) else {
                cx.fatal(format!(
                    "no return conversion from {source:?} to {target:?}"
                ));
                b.unreachable();
                return;
            };
            Some(coerced)
        }
    };
    if let Some(frame) = frame {
        let base = b.load_slot(frame.slot);
        let cleared = b.constant(0);
        b.store_slot(frame.slot, cleared);
        b.pop_frame(base, frame.kind);
    }
    b.return_value(result);
}
