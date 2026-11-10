//! The linkage ABI of a compiled function: one row per defined function,
//! frozen from its FunctionIR before the function set is partitioned into
//! independently compiled objects. Providers and consumers read the same row;
//! neither reconstructs a machine signature from its local body subset. The
//! late lowering stage of TIR reads it to admit a guarded direct call
//! (HF-821 D4).

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct NativeFunctionLinkageAbi {
    /// Target-neutral source ABI frozen from the owning FunctionIR before
    /// partitioning. Consumer declarations must match it exactly.
    pub source_signature: crate::ir::ExternFunctionSignature,
    /// Entry parameter custody frozen with the source signature (design 20
    /// §1.6). A consumer declaration must carry it exactly, so a caller in one
    /// object adopts precisely what the entry in another takes over.
    pub parameter_custody: Vec<crate::ir::ParameterCustody>,
    /// Exact machine-carrier types frozen before the function set is split
    /// into independently compiled objects.
    pub param_types: Vec<crate::tir::types::TirType>,
    /// `None` is a true native-void ABI. `Some` is the exact return carrier.
    pub return_type: Option<crate::tir::types::TirType>,
}
