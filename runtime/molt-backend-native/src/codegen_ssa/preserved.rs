//! Preserved operations (design: `docs/design/foundation/codegen_walker.md`,
//! W2): a TIR `Copy` whose `origin` names a source operation kind with its
//! own lowering. The walk's `Copy` arm asks each family table in turn; a
//! table answers false for a kind it does not own. A kind no table claims
//! is left to the backend, until every preserved family lowers here.

use super::frame::RecordPlacement;
use super::function::FunctionCx;
use super::walk::Position;
use crate::tir::ops::TirOp;

/// Every preserved kind a walker table lowers. A backend's own preserved
/// routing maps this pattern to no family of its own, so its exhaustive
/// match stays total while the walker claims these kinds first.
macro_rules! walker_preserved_kind_pattern {
    () => {
        molt_ir::OpKind::IterNextUnboxed
            | molt_ir::OpKind::IterNextUnpack2
            | molt_ir::OpKind::Len
            | molt_ir::OpKind::ListNew
            | molt_ir::OpKind::DictNew
            | molt_ir::OpKind::TupleNew
            | molt_ir::OpKind::SetNew
            | molt_ir::OpKind::FrozensetNew
            | molt_ir::OpKind::UnpackSequence
            | molt_ir::OpKind::BuiltinFunc
            | molt_ir::OpKind::FuncNew
            | molt_ir::OpKind::FuncNewClosure
            | molt_ir::OpKind::FunctionClosureBits
            | molt_ir::OpKind::AsyncgenNew
            | molt_ir::OpKind::CodeSlotSet
            | molt_ir::OpKind::CodeSlotsInit
            | molt_ir::OpKind::CallargsNew
            | molt_ir::OpKind::FrameLocals
            | molt_ir::OpKind::FrameLocalsSet
            | molt_ir::OpKind::Line
    };
}
pub(crate) use walker_preserved_kind_pattern;

impl<B: RecordPlacement> FunctionCx<'_, B> {
    /// Lower the preserved op `op` when a walker table owns its origin, and
    /// answer whether one did.
    pub(crate) fn preserved_op(&mut self, b: &mut B, _at: &Position<'_, B>, op: &TirOp) -> bool {
        // The pattern is the one list of walker-owned kinds: a table never
        // claims a kind a backend still routes.
        let Some(kind @ walker_preserved_kind_pattern!()) = op.origin else {
            return false;
        };
        self.container_preserved_op(b, op, kind) || self.callable_preserved_op(b, op, kind)
    }
}
