use super::*;

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
pub(crate) struct VarValue(pub(crate) Value);

#[cfg(feature = "native-backend")]
impl std::ops::Deref for VarValue {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn var_get(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: &str,
) -> Option<VarValue> {
    // The reserved singleton is a read operand, not a declared mutable home.
    // Preanalysis intentionally omits it from vars; every generic native
    // operand consumer must still receive its boxed value.
    if name == "none" {
        return Some(VarValue(builder.ins().iconst(types::I64, box_none())));
    }
    vars.get(name).map(|var| VarValue(builder.use_var(*var)))
}

#[cfg(feature = "native-backend")]
pub(crate) fn def_var_named(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: impl AsRef<str>,
    val: Value,
) {
    let name_ref = name.as_ref();
    if name_ref == "none" {
        return;
    }
    let var = *vars
        .get(name_ref)
        .unwrap_or_else(|| panic!("Var not found: {name_ref}"));
    if let Err(error) = builder.try_def_var(var, val) {
        let val_type = builder.func.dfg.value_type(val);
        panic!(
            "native variable representation mismatch for {name_ref}: value {val} has CLIF type {val_type}; {error}"
        );
    }
}

/// Seal a materialized block after its final predecessor has been emitted.
/// This is the sole writer of the function compiler's seal record: a block is
/// recorded only after Cranelift has actually sealed it. Repeated requests are
/// harmless, but a missing layout entry is a codegen error, not a deferred seal.
#[cfg(feature = "native-backend")]
#[inline]
pub(crate) fn seal_block_once(
    builder: &mut FunctionBuilder,
    sealed: &mut std::collections::BTreeSet<Block>,
    block: Block,
) {
    assert!(
        builder.func.layout.is_block_inserted(block),
        "native codegen cannot seal unmaterialized block {block:?}",
    );
    if !sealed.contains(&block) {
        builder.seal_block(block);
        sealed.insert(block);
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn jump_block(builder: &mut FunctionBuilder, target: Block, args: &[Value]) {
    let block_args: Vec<BlockArg> = args.iter().copied().map(BlockArg::from).collect();
    builder.ins().jump(target, &block_args);
}

#[cfg(feature = "native-backend")]
pub(crate) fn brif_block(
    builder: &mut FunctionBuilder,
    cond: Value,
    then_block: Block,
    then_args: &[Value],
    else_block: Block,
    else_args: &[Value],
) {
    let then_block_args: Vec<BlockArg> = then_args.iter().copied().map(BlockArg::from).collect();
    let else_block_args: Vec<BlockArg> = else_args.iter().copied().map(BlockArg::from).collect();
    builder.ins().brif(
        cond,
        then_block,
        &then_block_args,
        else_block,
        &else_block_args,
    );
}
