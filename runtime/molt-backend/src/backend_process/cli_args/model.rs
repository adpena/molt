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
    Daemon {
        socket_path: &'a str,
        emission_scope: Option<&'a str>,
    },
    /// `--native-batch-job-file JOB --output OBJECT`: one batch worker job, read
    /// from stdin when JOB is `-` or from a preserved replay file.
    #[cfg(feature = "native-backend")]
    NativeBatchJob {
        job_file: &'a str,
        emission_scope: Option<&'a str>,
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
    pub(crate) capture_origin: Option<&'a str>,
    pub(crate) emission_scope: Option<&'a str>,
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
}

/// `--target-triple` and `--native-output-kind`, or a daemon job's fields.
#[cfg(feature = "native-backend")]
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NativeTargetOptions {
    pub(crate) target_triple: Option<String>,
    pub(crate) output_kind: NativeArtifactKind,
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
        }
    }

    /// The document this target compiles and analyzes. Native and WASM link
    /// the runtime module catalog.
    pub(crate) fn admit_document(
        &self,
        document: BackendIrDocument,
    ) -> io::Result<BackendIrDocument> {
        match self {
            #[cfg(feature = "native-backend")]
            Self::Native(_) => Ok(document),
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
        }
    }
}
