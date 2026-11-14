pub use crate::wasm_options::{WasmCompileOptions, WasmProfile, WasmRuntimePollSlots};
mod backend;
mod class_def_layout;
mod compile_pipeline;
mod const_materialization;
mod constant_ops;
mod container_runtime_select;
mod context;
mod control_flow;
mod data_segments;
mod final_functions;
mod frame_locals;
pub(crate) mod frame_record;
mod function_emitter;
mod function_frame;
pub(crate) mod global_slot;
mod local_analysis;
pub(crate) mod method_site;
mod module_abi;
mod module_registry;
mod op_loop;
mod pending_work;
pub(crate) mod refcount;
mod state_dispatch;
mod task_runtime;
#[cfg(test)]
pub(crate) mod test_execution;
mod tir_pipeline;
mod trampoline_analysis;
pub use backend::{WasmBackend, WasmCompileDiagnostics, WasmCompileOutput, WasmNumericLaneStats};
#[cfg(test)]
pub(in crate::wasm) use frame_locals::WasmFrameLocalKind;
pub(in crate::wasm) use frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};

#[cfg(test)]
mod tests;
