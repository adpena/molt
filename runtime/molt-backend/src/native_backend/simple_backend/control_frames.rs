use super::*;

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeRebindStorageKind {
    BoxedI64,
    RawI64,
    RawBool,
    RawF64,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct MergeRebindSlot {
    pub(crate) slot: cranelift_codegen::ir::StackSlot,
    pub(crate) storage: MergeRebindStorageKind,
}

#[cfg(feature = "native-backend")]
pub(crate) struct IfFrame {
    pub(crate) else_block: Option<Block>,
    pub(crate) merge_block: Block,
    pub(crate) has_else: bool,
    pub(crate) then_terminal: bool,
    pub(crate) else_terminal: bool,
    pub(crate) phi_ops: Vec<(String, String, String)>,
    pub(crate) phi_params: Vec<Value>,
    pub(crate) merge_rebind_names: Vec<String>,
    pub(crate) merge_rebind_params: Vec<Value>,
    pub(crate) merge_rebind_slots: Vec<MergeRebindSlot>,
}

#[cfg(feature = "native-backend")]
pub(crate) struct LoopFrame {
    pub(crate) loop_block: Block,
    pub(crate) body_block: Block,
    pub(crate) after_block: Block,
    pub(crate) index_name: Option<String>,
    pub(crate) next_index: Option<Value>,
    /// True when the loop uses the linearized TIR path (no dedicated
    /// Cranelift loop block; counter flows through phi variables).
    /// `loop_end` must NOT decrement `loop_depth` for linearized loops
    /// because `loop_index_start` did not increment it.
    pub(crate) linearized: bool,
}
