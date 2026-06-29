#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeReturnAbi {
    I64,
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeImportSignature {
    pub(crate) name: &'static str,
    pub(crate) param_count: usize,
    pub(crate) return_abi: RuntimeReturnAbi,
}

pub(crate) const fn runtime_sig(
    name: &'static str,
    param_count: usize,
    return_abi: RuntimeReturnAbi,
) -> RuntimeImportSignature {
    RuntimeImportSignature {
        name,
        param_count,
        return_abi,
    }
}

pub(crate) const MOLT_DEC_REF: RuntimeImportSignature =
    runtime_sig("molt_dec_ref", 1, RuntimeReturnAbi::Void);
pub(crate) const MOLT_DEC_REF_OBJ: RuntimeImportSignature =
    runtime_sig("molt_dec_ref_obj", 1, RuntimeReturnAbi::Void);
pub(crate) const MOLT_INC_REF_OBJ: RuntimeImportSignature =
    runtime_sig("molt_inc_ref_obj", 1, RuntimeReturnAbi::Void);
pub(crate) const MOLT_TASK_NEW: RuntimeImportSignature =
    runtime_sig("molt_task_new", 3, RuntimeReturnAbi::I64);
pub(crate) const MOLT_CANCEL_TOKEN_GET_CURRENT: RuntimeImportSignature =
    runtime_sig("molt_cancel_token_get_current", 0, RuntimeReturnAbi::I64);
pub(crate) const MOLT_TASK_REGISTER_TOKEN_OWNED: RuntimeImportSignature =
    runtime_sig("molt_task_register_token_owned", 2, RuntimeReturnAbi::I64);
pub(crate) const MOLT_ASYNCGEN_NEW: RuntimeImportSignature =
    runtime_sig("molt_asyncgen_new", 1, RuntimeReturnAbi::I64);

#[cfg(test)]
pub(crate) const NATIVE_RUNTIME_HELPER_IMPORTS: &[RuntimeImportSignature] = &[
    MOLT_DEC_REF,
    MOLT_DEC_REF_OBJ,
    MOLT_INC_REF_OBJ,
    MOLT_TASK_NEW,
    MOLT_CANCEL_TOKEN_GET_CURRENT,
    MOLT_TASK_REGISTER_TOKEN_OWNED,
    MOLT_ASYNCGEN_NEW,
];

#[cfg(all(test, feature = "llvm"))]
pub(crate) const TRAMPOLINE_RUNTIME_IMPORTS: &[RuntimeImportSignature] = &[
    MOLT_INC_REF_OBJ,
    MOLT_TASK_NEW,
    MOLT_CANCEL_TOKEN_GET_CURRENT,
    MOLT_TASK_REGISTER_TOKEN_OWNED,
    MOLT_ASYNCGEN_NEW,
];
