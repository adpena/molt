//! Runtime imports the native helpers' tests name. Each entry's identity,
//! machine signature and execution facts come from the generated `RuntimeFn`
//! table (`molt_ir::runtime_fn_generated`).

#[cfg(test)]
use molt_ir::runtime_fn_generated::RuntimeFn;

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
