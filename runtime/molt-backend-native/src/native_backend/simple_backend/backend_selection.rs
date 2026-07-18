#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native_backend::simple_backend) enum NativeCodegenBackend {
    Cranelift,
    #[cfg(feature = "llvm")]
    Llvm,
}

impl NativeCodegenBackend {
    pub(in crate::native_backend::simple_backend) const fn uses_llvm(self) -> bool {
        #[cfg(feature = "llvm")]
        {
            matches!(self, Self::Llvm)
        }
        #[cfg(not(feature = "llvm"))]
        {
            false
        }
    }
}

/// Resolve the requested native code generator against compiled capabilities.
///
/// Backend selection and feature admission are one transaction: callers never
/// receive an LLVM selection from a binary that cannot execute it. Other
/// settings preserve the historical Cranelift default; validation of the
/// public CLI setting remains at the CLI boundary.
pub(in crate::native_backend::simple_backend) fn select_native_codegen_backend(
    setting: Option<&str>,
) -> NativeCodegenBackend {
    if setting != Some("llvm") {
        return NativeCodegenBackend::Cranelift;
    }

    #[cfg(feature = "llvm")]
    {
        NativeCodegenBackend::Llvm
    }
    #[cfg(not(feature = "llvm"))]
    {
        panic!(
            "MOLT_BACKEND=llvm requested but molt-backend was built without the llvm feature; rebuild with `--features llvm` or choose the Cranelift backend explicitly"
        );
    }
}
