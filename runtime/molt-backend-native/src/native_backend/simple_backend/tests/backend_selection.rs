use super::{NativeCodegenBackend, select_native_codegen_backend};

#[cfg(not(feature = "llvm"))]
#[test]
#[should_panic(
    expected = "MOLT_BACKEND=llvm requested but molt-backend was built without the llvm feature"
)]
fn llvm_backend_request_without_feature_fails_closed() {
    select_native_codegen_backend(Some("llvm"));
}

#[cfg(feature = "llvm")]
#[test]
fn llvm_backend_request_with_feature_selects_llvm() {
    let backend = select_native_codegen_backend(Some("llvm"));
    assert_eq!(backend, NativeCodegenBackend::Llvm);
    assert!(backend.uses_llvm());
}

#[test]
fn non_llvm_backend_settings_select_cranelift() {
    let default_backend = select_native_codegen_backend(None);
    assert_eq!(default_backend, NativeCodegenBackend::Cranelift);
    assert!(!default_backend.uses_llvm());
    assert_eq!(
        select_native_codegen_backend(Some("cranelift")),
        NativeCodegenBackend::Cranelift
    );
}
