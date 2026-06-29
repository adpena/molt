pub use crate::wasm_options::{WasmCompileOptions, WasmProfile};
mod backend;
pub(crate) mod body;
mod class_def_layout;
mod compile_pipeline;
mod const_materialization;
mod constant_ops;
mod container_runtime_select;
mod context;
mod control_flow;
mod data_segments;
mod frame_locals;
mod function_emitter;
mod function_frame;
pub(crate) mod lir_fast;
mod local_analysis;
mod method_ic_select;
mod module_abi;
mod multi_return_layout;
mod object_new_bound_select;
mod op_loop;
mod state_dispatch;
mod task_runtime;
mod tir_pipeline;
mod trampoline_analysis;
pub use backend::WasmBackend;
#[cfg(test)]
pub(in crate::wasm) use frame_locals::WasmFrameLocalKind;
pub(in crate::wasm) use frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};

#[cfg(test)]
mod tests;
