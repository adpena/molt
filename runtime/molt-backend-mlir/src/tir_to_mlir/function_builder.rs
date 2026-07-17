use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    dialect::func,
    ir::{
        Block, Identifier, Location, Region, RegionLike, Type,
        attribute::{StringAttribute, TypeAttribute},
        block::BlockLike,
        r#type::{FunctionType, IntegerType},
    },
};
use molt_backend::tir::{blocks::BlockId, function::TirFunction, types::TirType};

use super::{
    ops::emit_tir_op, terminators::emit_terminator, types::mlir_type_for_tir, values::ValueMap,
};

pub(super) fn build_func_op<'c>(
    tir_func: &TirFunction,
    ctx: &'c MlirContext,
    location: Location<'c>,
) -> Result<melior::ir::Operation<'c>, String> {
    let i64_type: Type<'c> = IntegerType::new(ctx, 64).into();
    let f64_type: Type<'c> = Type::float64(ctx);
    let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();

    // Map parameter and return types.
    let param_mlir_types: Vec<Type<'c>> = tir_func
        .param_types
        .iter()
        .map(|ty| mlir_type_for_tir(ctx, ty))
        .collect();
    let return_mlir_types: Vec<Type<'c>> = if matches!(tir_func.return_type, TirType::Never) {
        vec![]
    } else {
        vec![mlir_type_for_tir(ctx, &tir_func.return_type)]
    };

    let func_type = FunctionType::new(ctx, &param_mlir_types, &return_mlir_types);

    // Sort block IDs for deterministic emission. Entry block must be first.
    let mut block_ids: Vec<BlockId> = tir_func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    // Ensure entry block is first.
    if let Some(pos) = block_ids.iter().position(|b| *b == tir_func.entry_block) {
        block_ids.swap(0, pos);
    }

    // Phase 1: Create all MLIR blocks with their argument types.
    // We need block references before we can emit terminators that reference them.
    let mut mlir_blocks: Vec<Block<'c>> = Vec::with_capacity(block_ids.len());
    let mut block_index: HashMap<BlockId, usize> = HashMap::new();

    for (idx, &bid) in block_ids.iter().enumerate() {
        let tir_block = &tir_func.blocks[&bid];
        block_index.insert(bid, idx);

        // Entry block gets params from function signature, other blocks get
        // their block argument types.
        let arg_types: Vec<(Type<'c>, Location<'c>)> = if bid == tir_func.entry_block {
            param_mlir_types.iter().map(|&t| (t, location)).collect()
        } else {
            tir_block
                .args
                .iter()
                .map(|a| (mlir_type_for_tir(ctx, &a.ty), location))
                .collect()
        };
        mlir_blocks.push(Block::new(&arg_types));
    }

    // Phase 2: establish one function-wide SSA value authority before emitting
    // operations. TIR ValueIds are function-scoped and values defined in a
    // dominating block remain visible to successor blocks; rebuilding this map
    // per block silently discarded those definitions.
    let mut value_map = ValueMap::new(&tir_func.value_types);
    for (block_idx, &bid) in block_ids.iter().enumerate() {
        let tir_block = &tir_func.blocks[&bid];
        let block = &mlir_blocks[block_idx];
        for (index, arg) in tir_block.args.iter().enumerate() {
            value_map.insert(arg.id, block.argument(index).unwrap().into());
        }
    }

    // Phase 3: emit operations and terminators. Block references already exist,
    // while the shared map accumulates definitions in deterministic block order.
    for (blk_idx, &bid) in block_ids.iter().enumerate() {
        let tir_block = &tir_func.blocks[&bid];
        let block = &mlir_blocks[blk_idx];

        // Emit each op.
        for (op_index, op) in tir_block.ops.iter().enumerate() {
            emit_tir_op(
                ctx,
                block,
                op,
                &mut value_map,
                tir_func,
                i64_type,
                f64_type,
                i1_type,
                location,
            )
            .map_err(|error| {
                let prior = tir_block.ops[op_index.saturating_sub(5)..op_index]
                    .iter()
                    .map(|candidate| {
                        format!(
                            "{:?}(operands={:?}, results={:?})",
                            candidate.opcode, candidate.operands, candidate.results
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{} ^bb{} op #{op_index} {:?}(operands={:?}, results={:?}): {error}; preceding=[{prior}]",
                    tir_func.name, bid.0, op.opcode, op.operands, op.results
                )
            })?;
        }

        // Emit terminator.
        emit_terminator(
            ctx,
            block,
            &tir_block.terminator,
            &value_map,
            &block_index,
            &mlir_blocks,
            tir_func,
            i64_type,
            location,
        )
        .map_err(|error| {
            format!(
                "{} ^bb{} terminator {:?}: {error}",
                tir_func.name, bid.0, tir_block.terminator
            )
        })?;
    }

    // Phase 4: Assemble region and create func.func.
    let region = Region::new();
    for block in mlir_blocks {
        region.append_block(block);
    }

    // Add llvm.emit_c_interface attribute so the JIT can find the function
    // via the C calling convention wrapper.
    let emit_c_interface = (
        Identifier::new(ctx, "llvm.emit_c_interface"),
        melior::ir::attribute::Attribute::unit(ctx),
    );

    Ok(func::func(
        ctx,
        StringAttribute::new(ctx, &tir_func.name),
        TypeAttribute::new(func_type.into()),
        region,
        &[emit_c_interface],
        location,
    ))
}

pub(super) fn build_state_dispatch_runtime_declaration<'c>(
    ctx: &'c MlirContext,
    location: Location<'c>,
) -> melior::ir::Operation<'c> {
    let i64_type: Type<'c> = IntegerType::new(ctx, 64).into();
    let function_type = FunctionType::new(ctx, &[i64_type], &[i64_type]);
    let visibility = (
        Identifier::new(ctx, "sym_visibility"),
        StringAttribute::new(ctx, "private").into(),
    );
    func::func(
        ctx,
        StringAttribute::new(ctx, "molt_obj_get_state"),
        TypeAttribute::new(function_type.into()),
        Region::new(),
        &[visibility],
        location,
    )
}
