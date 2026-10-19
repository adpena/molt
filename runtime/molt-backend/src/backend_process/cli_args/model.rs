use crate::backend_process::input::IrFormat;
use molt_backend::BackendIrDocument;
use molt_backend::tir::target_info::TargetInfo;
#[cfg(feature = "native-backend")]
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::io;

#[cfg(feature = "native-backend")]
use crate::backend_process::NativeArtifactKind;

/// One backend invocation. A variant or target exists only in builds that
/// contain the code serving it, so a request this binary cannot serve fails
/// when it is parsed, before any IR is read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BackendCommand<'a> {
    /// `--features`: print the backends compiled into this binary.
    Features,
    /// `--daemon --socket PATH`: serve compile jobs on a Unix socket.
    #[cfg(molt_backend_daemon)]
    Daemon { socket_path: &'a str },
    /// `--native-batch-job-file JOB --output OBJECT`: one batch worker job, read
    /// from stdin when JOB is `-` or from a preserved replay file.
    #[cfg(feature = "native-backend")]
    NativeBatchJob {
        job_file: &'a str,
        output_path: &'a str,
    },
    /// Compile one IR document, or emit one function's TIR fact graph.
    Compile(BackendCompileArgs<'a>),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BackendCompileArgs<'a> {
    pub(crate) target: BackendTarget,
    pub(crate) output_path: Option<&'a str>,
    pub(crate) ir_file_path: Option<&'a str>,
    pub(crate) ir_format: IrFormat,
    pub(crate) fact_graph: Option<FactGraphCliRequest<'a>>,
}

/// `--fact-graph-output PATH --fact-graph-function NAME`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FactGraphCliRequest<'a> {
    pub(crate) output_path: &'a str,
    pub(crate) function_name: &'a str,
}

/// The backend a compile request or daemon job selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendTarget {
    #[cfg(feature = "native-backend")]
    Native(NativeTargetOptions),
    #[cfg(feature = "wasm-backend")]
    Wasm(WasmTargetOptions),
    #[cfg(feature = "luau-backend")]
    Luau,
    #[cfg(feature = "rust-backend")]
    Rust,
}

/// `--target-triple` and `--native-output-kind`, or a daemon job's fields.
#[cfg(feature = "native-backend")]
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NativeTargetOptions {
    pub(crate) target_triple: Option<String>,
    pub(crate) output_kind: NativeArtifactKind,
}

/// The `--wasm-*` layout overrides, or a daemon job's fields. An absent
/// override keeps `WasmCompileOptions::default()`, which reads the
/// `MOLT_WASM_*` environment when the module compiles.
#[cfg(feature = "wasm-backend")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WasmTargetOptions {
    pub(crate) link_relocs: bool,
    pub(crate) data_base: Option<u32>,
    pub(crate) table_base: Option<u32>,
    pub(crate) split_runtime_app_table_base: Option<u32>,
}

#[cfg(feature = "wasm-backend")]
impl WasmTargetOptions {
    /// The one conversion the CLI and the daemon share, so a request builds
    /// the same module in both modes.
    pub(crate) fn compile_options(self) -> molt_backend::WasmCompileOptions {
        let mut options = molt_backend::WasmCompileOptions::default();
        options.reloc_enabled |= self.link_relocs;
        if let Some(data_base) = self.data_base {
            options.data_base = data_base;
        }
        if let Some(table_base) = self.table_base {
            options.table_base = table_base;
        }
        if let Some(base) = self.split_runtime_app_table_base {
            options.split_runtime_app_table_base = Some(base);
        }
        options
    }
}

impl BackendTarget {
    /// Where the artifact goes when the request names no `--output`.
    pub(crate) fn default_output_path(&self) -> &'static str {
        match self {
            #[cfg(feature = "native-backend")]
            Self::Native(options) => match options.output_kind {
                NativeArtifactKind::Object => "dist/output.o",
                NativeArtifactKind::Archive => "dist/output.a",
            },
            #[cfg(feature = "wasm-backend")]
            Self::Wasm(_) => "dist/output.wasm",
            #[cfg(feature = "luau-backend")]
            Self::Luau => "dist/output.luau",
            #[cfg(feature = "rust-backend")]
            Self::Rust => "dist/output.rs",
        }
    }

    /// The document this target compiles and analyzes. Native and WASM link
    /// the runtime module catalog; source emitters do not ship it, so they
    /// refuse a catalog instead of dropping it. Luau also drops the runtime
    /// bootstrap before its fact graphs and codegen; the Rust emitter does
    /// that after its SimpleIR pipeline.
    pub(crate) fn admit_document(
        &self,
        document: BackendIrDocument,
    ) -> io::Result<BackendIrDocument> {
        match self {
            #[cfg(feature = "native-backend")]
            Self::Native(_) => Ok(document),
            #[cfg(feature = "wasm-backend")]
            Self::Wasm(_) => Ok(document),
            #[cfg(feature = "luau-backend")]
            Self::Luau => {
                let mut document = without_module_catalog(document)?;
                document.ir.tree_shake_source_emission();
                Ok(document)
            }
            #[cfg(feature = "rust-backend")]
            Self::Rust => without_module_catalog(document),
        }
    }

    /// The target description a fact graph is computed for.
    pub(crate) fn fact_graph_target_info(&self) -> io::Result<TargetInfo> {
        match self {
            #[cfg(feature = "native-backend")]
            Self::Native(_) => Ok(
                match backend_env::var(BackendEnv::MOLT_BACKEND).ok().as_deref() {
                    Some("llvm") => TargetInfo::llvm_release_fast(),
                    _ => TargetInfo::native_release_fast(),
                },
            ),
            #[cfg(feature = "wasm-backend")]
            Self::Wasm(_) => Ok(TargetInfo::wasm_release_fast()),
            #[cfg(feature = "luau-backend")]
            Self::Luau => Ok(TargetInfo::luau_release_fast()),
            #[cfg(feature = "rust-backend")]
            Self::Rust => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fact graph emission does not support the rust target",
            )),
        }
    }
}

#[cfg(any(feature = "luau-backend", feature = "rust-backend"))]
fn without_module_catalog(document: BackendIrDocument) -> io::Result<BackendIrDocument> {
    if document.module_registry.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "module_registry requires a native or WASM runtime-backed target",
        ));
    }
    Ok(document)
}
