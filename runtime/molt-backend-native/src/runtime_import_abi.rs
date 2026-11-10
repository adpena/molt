//! Machine return shapes of runtime imports. Each entry's identity, word
//! arity and return contract come from the generated `RuntimeFn` table
//! (`molt_ir::runtime_fn_generated`); this module only projects them.

#[cfg(any(test, feature = "llvm"))]
use molt_ir::runtime_fn_generated::RuntimeFn;

/// Only the LLVM backend declares imports by machine return shape.
#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeReturnAbi {
    I64,
    Void,
}

#[cfg(feature = "llvm")]
impl RuntimeReturnAbi {
    pub(crate) const fn of(function: RuntimeFn) -> Self {
        if function.returns_word() {
            Self::I64
        } else {
            Self::Void
        }
    }
}

#[cfg(test)]
pub(crate) const NATIVE_RUNTIME_HELPER_IMPORTS: &[RuntimeFn] = &[
    RuntimeFn::DecRefObj,
    RuntimeFn::IncRefObj,
    RuntimeFn::RefcountAdmission,
    RuntimeFn::TaskNew,
    RuntimeFn::CancelTokenGetCurrent,
    RuntimeFn::TaskRegisterExecution,
    RuntimeFn::AsyncgenNew,
];

#[cfg(all(test, feature = "llvm"))]
pub(crate) const TRAMPOLINE_RUNTIME_IMPORTS: &[RuntimeFn] = &[
    RuntimeFn::IncRefObj,
    RuntimeFn::TaskNew,
    RuntimeFn::CancelTokenGetCurrent,
    RuntimeFn::TaskRegisterExecution,
    RuntimeFn::AsyncgenNew,
];
