mod locals;
mod policy;
#[cfg(test)]
mod tests;

pub(in crate::wasm) use locals::WasmLiteralScratchLocals;
pub(in crate::wasm) use policy::WasmLiteralScratchPolicy;
