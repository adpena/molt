use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmConstRawIntEffect;
use molt_codegen_abi::fits_inline_int;
use std::collections::BTreeMap;

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn apply_raw_int_effect(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
        known_raw_ints: &mut BTreeMap<u32, i64>,
    ) {
        match self.raw_int_effect() {
            // A known raw int stands for an inline int: its readers skip the
            // tag guard, unbox it as `i64.const` and box it with `box_int`. A
            // constant outside the inline range is a heap int, so it is not
            // known raw (a folded product can be such a constant).
            WasmConstRawIntEffect::SetInt => {
                let val = op.value.expect("raw-int const value");
                if fits_inline_int(val) {
                    let out = op.out.as_ref().expect("raw-int const out");
                    known_raw_ints.insert(locals.result_slot(out), val);
                } else {
                    forget_output_raw_int(op, locals, known_raw_ints);
                }
            }
            WasmConstRawIntEffect::Clear => forget_output_raw_int(op, locals, known_raw_ints),
        }
    }

    pub(in crate::wasm) fn clear_raw_int_output(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
        known_raw_ints: &mut BTreeMap<u32, i64>,
    ) {
        let _ = self;
        forget_output_raw_int(op, locals, known_raw_ints);
    }
}

fn forget_output_raw_int(
    op: &OpIR,
    locals: &WasmFrameLocals,
    known_raw_ints: &mut BTreeMap<u32, i64>,
) {
    if let Some(out) = op.out.as_ref() {
        known_raw_ints.remove(&locals.result_slot(out));
    }
}
