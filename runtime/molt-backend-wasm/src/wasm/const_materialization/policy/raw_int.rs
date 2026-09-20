use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmConstRawIntEffect;
use std::collections::BTreeMap;

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn apply_raw_int_effect(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
        known_raw_ints: &mut BTreeMap<u32, i64>,
    ) {
        match self.raw_int_effect() {
            WasmConstRawIntEffect::SetInt => {
                let out = op.out.as_ref().expect("raw-int const out");
                let local_idx = locals.result_slot(out);
                let val = op.value.expect("raw-int const value");
                known_raw_ints.insert(local_idx, val);
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
