pub use crate::wasm_options::{WasmCompileOptions, WasmProfile};
mod backend;
pub(crate) mod body;
mod call_site_abi;
mod class_def_layout;
mod compile_pipeline;
mod const_materialization;
mod constant_ops;
mod context;
mod control_flow;
mod data_segments;
mod frame_locals;
mod function_emitter;
mod function_frame;
pub(crate) mod lir_fast;
mod local_analysis;
mod module_abi;
mod multi_return_layout;
mod op_loop;
mod state_dispatch;
mod tir_pipeline;
mod trampoline_analysis;
pub use backend::WasmBackend;
#[cfg(test)]
pub(in crate::wasm) use frame_locals::WasmFrameLocalKind;
pub(in crate::wasm) use frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};

#[cfg(test)]
mod tests;
