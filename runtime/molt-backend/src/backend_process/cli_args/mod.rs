mod model;
mod parse;

#[cfg(feature = "native-backend")]
pub(crate) use model::NativeTargetOptions;
#[cfg(feature = "wasm-backend")]
pub(crate) use model::WasmTargetOptions;
pub(crate) use model::{BackendCommand, BackendCompileArgs, BackendTarget};
