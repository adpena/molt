mod emit;
mod policy;

pub(crate) use emit::{WasmConstMaterialization, WasmConstMaterializationScratch};
pub(crate) use policy::WasmConstOpPolicy;
