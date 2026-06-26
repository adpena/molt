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

/// Seal a block only if it hasn't been sealed yet. Prevents the
/// `!self.is_sealed(block)` assertion panic in Cranelift's SSA builder
/// when multiple code paths attempt to seal the same block.
#[cfg(feature = "native-backend")]
#[inline]
pub(crate) fn seal_block_once(
    builder: &mut FunctionBuilder,
    sealed: &mut std::collections::BTreeSet<Block>,
    block: Block,
) {
    if sealed.insert(block) && builder.func.layout.is_block_inserted(block) {
        builder.seal_block(block);
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
