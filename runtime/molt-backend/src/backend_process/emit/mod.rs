use molt_backend::SimpleIR;
use std::io;

use super::cli_args::WasmCliOptions;
use super::io_limits::{BackendOutputKind, ensure_output_parent_dir, resolve_backend_output_path};

#[cfg(feature = "luau-backend")]
mod luau;
#[cfg(any(feature = "luau-backend", test))]
mod luau_pipeline;
#[cfg(feature = "native-backend")]
mod native;
#[cfg(feature = "rust-backend")]
mod rust;
#[cfg(feature = "wasm-backend")]
mod wasm;

#[cfg(feature = "luau-backend")]
use luau::emit_luau_target;
#[cfg(test)]
pub(crate) use luau_pipeline::run_luau_tir_module_pipeline;
#[cfg(feature = "native-backend")]
use native::emit_native_target;
#[cfg(feature = "rust-backend")]
use rust::emit_rust_target;
#[cfg(all(feature = "rust-backend", test))]
pub(crate) use rust::rust_source_for_ir;
#[cfg(feature = "wasm-backend")]
use wasm::emit_wasm_target;
#[cfg(all(any(unix, test), feature = "wasm-backend"))]
pub(crate) use wasm::validate_wasm_module_catalog;

pub(crate) struct BackendTargetEmitRequest<'a> {
    pub(crate) ir: SimpleIR,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) module_registry: Option<molt_backend::ModuleRegistryIR>,
    pub(crate) output_path: Option<&'a str>,
    pub(crate) output_kind: BackendOutputKind,
    #[cfg_attr(not(feature = "luau-backend"), allow(dead_code))]
    pub(crate) use_ir_pipeline: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<&'a str>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_options: WasmCliOptions,
}

pub(crate) fn emit_backend_target(request: BackendTargetEmitRequest<'_>) -> io::Result<()> {
    let output_file = resolve_backend_output_path(request.output_path, request.output_kind);
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to create backend output parent for '{}': {}",
                output_file, err
            ),
        )
    })?;

    match request.output_kind {
        BackendOutputKind::Luau => {
            #[cfg(feature = "luau-backend")]
            {
                let mut ir = request.ir;
                emit_luau_target(&mut ir, output_file, request.use_ir_pipeline)?;
                Ok(())
            }
            #[cfg(not(feature = "luau-backend"))]
            {
                drop(request.ir);
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without luau-backend support; rebuild with: cargo build -p molt-backend --features luau-backend",
                ))
            }
        }
        BackendOutputKind::Rust => {
            #[cfg(feature = "rust-backend")]
            {
                emit_rust_target(&request.ir, output_file)?;
                Ok(())
            }
            #[cfg(not(feature = "rust-backend"))]
            {
                drop(request.ir);
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without rust-backend support; rebuild with: cargo build -p molt-backend --features rust-backend",
                ))
            }
        }
        BackendOutputKind::Wasm => {
            #[cfg(feature = "wasm-backend")]
            {
                emit_wasm_target(
                    request.ir,
                    request.module_registry,
                    output_file,
                    request.wasm_options,
                )?;
                Ok(())
            }
            #[cfg(not(feature = "wasm-backend"))]
            {
                drop(request.ir);
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend",
                ))
            }
        }
        BackendOutputKind::Native => {
            #[cfg(feature = "native-backend")]
            {
                emit_native_target(
                    request.ir,
                    request.module_registry,
                    output_file,
                    request.target_triple,
                )?;
                Ok(())
            }
            #[cfg(not(feature = "native-backend"))]
            {
                drop(request.ir);
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend",
                ))
            }
        }
    }
}
