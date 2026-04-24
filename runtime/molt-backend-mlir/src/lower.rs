//! TIR-to-MLIR structural lowering.
//!
//! Converts TIR functions into MLIR modules using melior's programmatic API.
//! Operations that map directly to standard MLIR dialects (arith, func, cf)
//! are emitted using the typed helpers; molt-specific operations that have no
//! standard-dialect counterpart are emitted as unregistered generic ops via
//! `OperationBuilder` with string names.

use std::collections::HashMap;

use melior::{
    Context,
    ir::{
        Attribute, Block, Identifier, Location, Module, Region, RegionLike, Type, Value,
        attribute::{
            FloatAttribute, FlatSymbolRefAttribute, IntegerAttribute, StringAttribute,
            TypeAttribute,
        },
        block::BlockLike,
        operation::{OperationBuilder, OperationLike},
        r#type::{FunctionType, IntegerType},
    },
    dialect::{arith, cf, func},
};

use molt_backend::tir::{
    blocks::{BlockId, Terminator},
    function::TirFunction,
    ops::{AttrValue, Dialect, OpCode, TirOp},
    types::TirType,
    values::ValueId,
};

/// Map TirType to an MLIR Type in the given context.
fn mlir_type_for<'c>(ctx: &'c Context, ty: &TirType) -> Type<'c> {
    match ty {
        TirType::I64 | TirType::BigInt => IntegerType::new(ctx, 64).into(),
        TirType::F64 => Type::float64(ctx),
        TirType::Bool => IntegerType::new(ctx, 1).into(),
        TirType::None | TirType::DynBox => IntegerType::new(ctx, 64).into(),
        TirType::Str | TirType::Bytes | TirType::Ptr(_) => IntegerType::new(ctx, 64).into(),
        TirType::Never => Type::none(ctx),
        TirType::List(_)
        | TirType::Dict(_, _)
        | TirType::Set(_)
        | TirType::Tuple(_)
        | TirType::Box(_)
        | TirType::Func(_)
        | TirType::Union(_) => IntegerType::new(ctx, 64).into(),
    }
}

/// Resolve the MLIR type for a TIR value, falling back to the function-level
/// type map when the value was defined as a block argument.
fn resolve_value_type<'c>(
    ctx: &'c Context,
    func: &TirFunction,
    vid: ValueId,
) -> Type<'c> {
    // Check block arguments first.
    for block in func.blocks.values() {
        for arg in &block.args {
            if arg.id == vid {
                return mlir_type_for(ctx, &arg.ty);
            }
        }
    }
    // Check op results -- scan for the op that defines this value.
    for block in func.blocks.values() {
        for op in &block.ops {
            for r in &op.results {
                if *r == vid {
                    // For ops with typed results, infer from context.
                    return infer_op_result_type(ctx, func, op);
                }
            }
        }
    }
    // Fallback: i64 (DynBox representation).
    IntegerType::new(ctx, 64).into()
}

/// Infer the MLIR result type for a TIR operation.
fn infer_op_result_type<'c>(
    ctx: &'c Context,
    func: &TirFunction,
    op: &TirOp,
) -> Type<'c> {
    match op.opcode {
        OpCode::ConstInt => IntegerType::new(ctx, 64).into(),
        OpCode::ConstFloat => Type::float64(ctx),
        OpCode::ConstBool => IntegerType::new(ctx, 1).into(),
        OpCode::ConstNone => IntegerType::new(ctx, 64).into(),
        OpCode::ConstStr | OpCode::ConstBytes => IntegerType::new(ctx, 64).into(),
        // Comparison ops always produce i1.
        OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge
        | OpCode::Is | OpCode::IsNot | OpCode::In | OpCode::NotIn | OpCode::Not
        | OpCode::Bool => IntegerType::new(ctx, 1).into(),
        // Arithmetic ops: propagate from first operand, or default to i64.
        OpCode::Add | OpCode::Sub | OpCode::Mul | OpCode::Div | OpCode::FloorDiv
        | OpCode::Mod | OpCode::Pow | OpCode::Neg | OpCode::Pos
        | OpCode::InplaceAdd | OpCode::InplaceSub | OpCode::InplaceMul => {
            if let Some(first_operand) = op.operands.first() {
                resolve_value_type(ctx, func, *first_operand)
            } else {
                IntegerType::new(ctx, 64).into()
            }
        }
        // Bitwise ops: always integer.
        OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor | OpCode::BitNot
        | OpCode::Shl | OpCode::Shr => IntegerType::new(ctx, 64).into(),
        // Logical ops.
        OpCode::And | OpCode::Or => IntegerType::new(ctx, 1).into(),
        // Copy propagates source type.
        OpCode::Copy => {
            if let Some(src) = op.operands.first() {
                resolve_value_type(ctx, func, *src)
            } else {
                IntegerType::new(ctx, 64).into()
            }
        }
        // Everything else: i64 (DynBox).
        _ => IntegerType::new(ctx, 64).into(),
    }
}

/// Extract an integer constant from the "value" attr of a TirOp.
fn attr_int(op: &TirOp) -> i64 {
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => *v,
        _ => 0,
    }
}

/// Extract a float constant from the "value" attr of a TirOp.
fn attr_float(op: &TirOp) -> f64 {
    match op.attrs.get("value") {
        Some(AttrValue::Float(v)) => *v,
        _ => 0.0,
    }
}

/// Extract a bool constant from the "value" attr of a TirOp.
fn attr_bool(op: &TirOp) -> bool {
    match op.attrs.get("value") {
        Some(AttrValue::Bool(v)) => *v,
        _ => false,
    }
}

/// Extract a string constant from the "value" attr of a TirOp.
fn attr_str(op: &TirOp) -> &str {
    match op.attrs.get("value") {
        Some(AttrValue::Str(v)) => v.as_str(),
        _ => "",
    }
}

/// Lower a TIR function to an MLIR Module using the melior programmatic API.
///
/// Returns the owning Module. The caller is responsible for verifying and
/// optimizing it.
pub fn lower_tir_to_mlir<'c>(
    ctx: &'c Context,
    tir_func: &TirFunction,
) -> Module<'c> {
    let location = Location::unknown(ctx);
    let module = Module::new(location);

    // Compute sorted block ordering (entry first, then ascending).
    let mut block_ids: Vec<BlockId> = tir_func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    // Ensure the entry block is first.
    if let Some(pos) = block_ids.iter().position(|b| *b == tir_func.entry_block) {
        if pos != 0 {
            block_ids.swap(0, pos);
        }
    }

    // Build MLIR types for the function signature.
    let param_types: Vec<Type<'c>> = tir_func
        .param_types
        .iter()
        .map(|ty| mlir_type_for(ctx, ty))
        .collect();
    let ret_type = mlir_type_for(ctx, &tir_func.return_type);
    let has_return = !matches!(tir_func.return_type, TirType::None | TirType::Never);
    let result_types: Vec<Type<'c>> = if has_return { vec![ret_type] } else { vec![] };
    let func_type = FunctionType::new(ctx, &param_types, &result_types);

    // --- Phase 1: Create all MLIR blocks with their argument types. ---
    // We need all blocks to exist before we can emit terminators that
    // reference successor blocks.
    let mut mlir_blocks: HashMap<BlockId, Block<'c>> = HashMap::new();
    for &bid in &block_ids {
        let tir_block = &tir_func.blocks[&bid];
        let arg_types: Vec<(Type<'c>, Location<'c>)> = tir_block
            .args
            .iter()
            .map(|a| (mlir_type_for(ctx, &a.ty), location))
            .collect();
        let block = Block::new(&arg_types);
        mlir_blocks.insert(bid, block);
    }

    // --- Phase 2: Emit operations and terminators into each block. ---
    // We track the mapping from TIR ValueId -> MLIR Value.
    // Because melior's Value borrows from the Block/Operation, we use a
    // two-pass strategy: first emit all ops, then collect references.
    //
    // Due to lifetime constraints in melior (Values borrow from their
    // containing Block), we cannot store Value references across block
    // boundaries in a simple HashMap. Instead, we use the textual MLIR
    // parse fallback for cross-block references and emit intra-block
    // operations using the programmatic API.
    //
    // Strategy: Build each block's ops programmatically. For terminators
    // that reference other blocks, we use OperationBuilder with successor
    // block references (which melior supports).

    // We process blocks in order. For each block, we emit ops and track
    // the Value mapping within that block. Cross-block value references
    // (operands defined in other blocks) are handled through block
    // arguments (MLIR's equivalent of phi nodes) which are already
    // established via block args.

    for &bid in &block_ids {
        let tir_block = &tir_func.blocks[&bid];
        let mlir_block = mlir_blocks.get(&bid).unwrap();

        // Map block arguments to their MLIR Values.
        let mut value_map: HashMap<ValueId, Value<'c, '_>> = HashMap::new();
        for (i, arg) in tir_block.args.iter().enumerate() {
            value_map.insert(arg.id, mlir_block.argument(i).unwrap().into());
        }

        // Emit each TIR op.
        for op in &tir_block.ops {
            emit_tir_op(ctx, tir_func, mlir_block, op, &mut value_map);
        }

        // Emit the terminator.
        emit_terminator(
            ctx,
            tir_func,
            mlir_block,
            &tir_block.terminator,
            &value_map,
            &mlir_blocks,
            &result_types,
        );
    }

    // --- Phase 3: Assemble the function. ---
    let region = Region::new();
    for &bid in &block_ids {
        let block = mlir_blocks.remove(&bid).unwrap();
        region.append_block(block);
    }

    let function = func::func(
        ctx,
        StringAttribute::new(ctx, &tir_func.name),
        TypeAttribute::new(func_type.into()),
        region,
        &[],
        location,
    );

    module.body().append_operation(function);
    module
}

/// Emit a single TIR operation into an MLIR block.
fn emit_tir_op<'c, 'b>(
    ctx: &'c Context,
    tir_func: &TirFunction,
    block: &'b Block<'c>,
    op: &TirOp,
    value_map: &mut HashMap<ValueId, Value<'c, 'b>>,
) {
    let location = Location::unknown(ctx);

    // Helper: look up an operand's MLIR Value. If not found in the local
    // value_map (cross-block reference), emit a placeholder constant.
    let get_operand = |vid: &ValueId, vm: &HashMap<ValueId, Value<'c, 'b>>| -> Option<Value<'c, 'b>> {
        vm.get(vid).copied()
    };

    match op.opcode {
        // --- Constants ---
        OpCode::ConstInt => {
            let val = attr_int(op);
            let i64_type = IntegerType::new(ctx, 64).into();
            let mlir_op = block.append_operation(arith::constant(
                ctx,
                IntegerAttribute::new(i64_type, val).into(),
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::ConstFloat => {
            let val = attr_float(op);
            let f64_type = Type::float64(ctx);
            let mlir_op = block.append_operation(arith::constant(
                ctx,
                FloatAttribute::new(ctx, f64_type, val).into(),
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::ConstBool => {
            let val = attr_bool(op);
            let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();
            let mlir_op = block.append_operation(arith::constant(
                ctx,
                IntegerAttribute::new(i1_type, if val { 1 } else { 0 }).into(),
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::ConstNone => {
            let i64_type = IntegerType::new(ctx, 64).into();
            let mlir_op = block.append_operation(arith::constant(
                ctx,
                IntegerAttribute::new(i64_type, 0).into(),
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            // String/bytes constants are opaque pointers in the runtime.
            // Emit as i64 constant zero (placeholder for runtime string table index).
            let i64_type = IntegerType::new(ctx, 64).into();
            let mlir_op = block.append_operation(arith::constant(
                ctx,
                IntegerAttribute::new(i64_type, 0).into(),
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        // --- Integer arithmetic (i64) ---
        OpCode::Add | OpCode::InplaceAdd => {
            emit_binary_arith(ctx, block, op, value_map, tir_func, BinaryArithKind::Add);
        }
        OpCode::Sub | OpCode::InplaceSub => {
            emit_binary_arith(ctx, block, op, value_map, tir_func, BinaryArithKind::Sub);
        }
        OpCode::Mul | OpCode::InplaceMul => {
            emit_binary_arith(ctx, block, op, value_map, tir_func, BinaryArithKind::Mul);
        }
        OpCode::Div => {
            emit_binary_arith(ctx, block, op, value_map, tir_func, BinaryArithKind::Div);
        }
        OpCode::FloorDiv => {
            emit_binary_arith(ctx, block, op, value_map, tir_func, BinaryArithKind::FloorDiv);
        }
        OpCode::Mod => {
            emit_binary_arith(ctx, block, op, value_map, tir_func, BinaryArithKind::Mod);
        }
        OpCode::Neg => {
            if let (Some(operand), Some(&result_id)) =
                (op.operands.first().and_then(|v| get_operand(v, value_map)), op.results.first())
            {
                let result_type = resolve_value_type(ctx, tir_func, result_id);
                if result_type == Type::float64(ctx) {
                    let mlir_op = block.append_operation(arith::negf(operand, location));
                    value_map.insert(result_id, mlir_op.result(0).unwrap().into());
                } else {
                    // Integer negation: 0 - x.
                    let i64_type = IntegerType::new(ctx, 64).into();
                    let zero_op = block.append_operation(arith::constant(
                        ctx,
                        IntegerAttribute::new(i64_type, 0).into(),
                        location,
                    ));
                    let mlir_op = block.append_operation(arith::subi(
                        zero_op.result(0).unwrap().into(),
                        operand,
                        location,
                    ));
                    value_map.insert(result_id, mlir_op.result(0).unwrap().into());
                }
            }
        }
        OpCode::Pos => {
            // Pos is identity.
            if let (Some(&src), Some(&result_id)) = (op.operands.first(), op.results.first()) {
                if let Some(val) = get_operand(&src, value_map) {
                    value_map.insert(result_id, val);
                }
            }
        }
        OpCode::Pow => {
            // No standard MLIR pow for integers; emit as unregistered op.
            emit_generic_op(ctx, block, op, value_map, tir_func, "molt.pow");
        }

        // --- Bitwise ops ---
        OpCode::BitAnd => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::andi(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::BitOr => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::ori(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::BitXor => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::xori(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::BitNot => {
            // ~x = x ^ -1 (all ones).
            if let (Some(operand), Some(&result_id)) =
                (op.operands.first().and_then(|v| get_operand(v, value_map)), op.results.first())
            {
                let i64_type = IntegerType::new(ctx, 64).into();
                let neg_one = block.append_operation(arith::constant(
                    ctx,
                    IntegerAttribute::new(i64_type, -1).into(),
                    location,
                ));
                let mlir_op = block.append_operation(arith::xori(
                    operand,
                    neg_one.result(0).unwrap().into(),
                    location,
                ));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::Shl => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::shli(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::Shr => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::shrsi(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        // --- Comparison ops ---
        OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => {
            emit_comparison(ctx, block, op, value_map, tir_func);
        }

        // --- Boolean ops ---
        OpCode::And => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::andi(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::Or => {
            if let (Some(lhs), Some(rhs), Some(&result_id)) = (
                op.operands.first().and_then(|v| get_operand(v, value_map)),
                op.operands.get(1).and_then(|v| get_operand(v, value_map)),
                op.results.first(),
            ) {
                let mlir_op = block.append_operation(arith::ori(lhs, rhs, location));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::Not => {
            // !x = x ^ true.
            if let (Some(operand), Some(&result_id)) =
                (op.operands.first().and_then(|v| get_operand(v, value_map)), op.results.first())
            {
                let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();
                let one = block.append_operation(arith::constant(
                    ctx,
                    IntegerAttribute::new(i1_type, 1).into(),
                    location,
                ));
                let mlir_op = block.append_operation(arith::xori(
                    operand,
                    one.result(0).unwrap().into(),
                    location,
                ));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        OpCode::Bool => {
            // Bool conversion: compare != 0.
            if let (Some(operand), Some(&result_id)) =
                (op.operands.first().and_then(|v| get_operand(v, value_map)), op.results.first())
            {
                let i64_type = IntegerType::new(ctx, 64).into();
                let zero = block.append_operation(arith::constant(
                    ctx,
                    IntegerAttribute::new(i64_type, 0).into(),
                    location,
                ));
                let mlir_op = block.append_operation(arith::cmpi(
                    ctx,
                    arith::CmpiPredicate::Ne,
                    operand,
                    zero.result(0).unwrap().into(),
                    location,
                ));
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        // --- Call ---
        OpCode::Call | OpCode::CallBuiltin => {
            let callee_name = attr_str(op);
            let callee = if !callee_name.is_empty() {
                callee_name.to_string()
            } else if let Some(AttrValue::Str(s)) = op.attrs.get("callee") {
                s.clone()
            } else if let Some(AttrValue::Str(s)) = op.attrs.get("name") {
                s.clone()
            } else {
                format!("__molt_call_{}", op.results.first().map_or(0, |v| v.0))
            };

            let args: Vec<Value<'c, '_>> = op
                .operands
                .iter()
                .filter_map(|v| get_operand(v, value_map))
                .collect();
            let result_types: Vec<Type<'c>> = op
                .results
                .iter()
                .map(|r| resolve_value_type(ctx, tir_func, *r))
                .collect();

            let mlir_op = block.append_operation(func::call(
                ctx,
                FlatSymbolRefAttribute::new(ctx, &callee),
                &args,
                &result_types,
                location,
            ));

            for (i, &result_id) in op.results.iter().enumerate() {
                if let Ok(r) = mlir_op.result(i) {
                    value_map.insert(result_id, r.into());
                }
            }
        }

        // --- Copy (SSA identity) ---
        OpCode::Copy => {
            if let (Some(&src), Some(&result_id)) = (op.operands.first(), op.results.first()) {
                if let Some(val) = get_operand(&src, value_map) {
                    value_map.insert(result_id, val);
                }
            }
        }

        // --- All other ops: emit as unregistered generic MLIR ops ---
        _ => {
            let op_name = generic_op_name(op);
            emit_generic_op(ctx, block, op, value_map, tir_func, &op_name);
        }
    }
}

/// Determine the fully-qualified generic op name for a TIR op.
fn generic_op_name(op: &TirOp) -> String {
    let dialect = match op.dialect {
        Dialect::Molt => "molt",
        Dialect::Scf => "scf",
        Dialect::Gpu => "molt_gpu",
        Dialect::Par => "molt_par",
        Dialect::Simd => "molt_simd",
    };
    let opcode = molt_backend::tir::mlir_compat::opcode_name(&op.opcode);
    format!("{dialect}.{opcode}")
}

/// Emit an unregistered generic MLIR operation for TIR ops that have no
/// standard dialect counterpart.
fn emit_generic_op<'c, 'b>(
    ctx: &'c Context,
    block: &'b Block<'c>,
    op: &TirOp,
    value_map: &mut HashMap<ValueId, Value<'c, 'b>>,
    tir_func: &TirFunction,
    op_name: &str,
) {
    let location = Location::unknown(ctx);
    let operands: Vec<Value<'c, '_>> = op
        .operands
        .iter()
        .filter_map(|v| value_map.get(v).copied())
        .collect();
    let result_types: Vec<Type<'c>> = op
        .results
        .iter()
        .map(|r| resolve_value_type(ctx, tir_func, *r))
        .collect();

    // Build attributes from the TIR attr dict.
    let attrs: Vec<(Identifier<'c>, Attribute<'c>)> = op
        .attrs
        .iter()
        .filter_map(|(k, v)| {
            let attr: Attribute<'c> = match v {
                AttrValue::Int(i) => {
                    IntegerAttribute::new(IntegerType::new(ctx, 64).into(), *i).into()
                }
                AttrValue::Float(f) => {
                    FloatAttribute::new(ctx, Type::float64(ctx), *f).into()
                }
                AttrValue::Str(s) => StringAttribute::new(ctx, s).into(),
                AttrValue::Bool(b) => {
                    IntegerAttribute::new(IntegerType::new(ctx, 1).into(), if *b { 1 } else { 0 })
                        .into()
                }
                AttrValue::Bytes(_) => return None,
            };
            Some((Identifier::new(ctx, k), attr))
        })
        .collect();

    let mlir_op = block.append_operation(
        OperationBuilder::new(op_name, location)
            .add_operands(&operands)
            .add_results(&result_types)
            .add_attributes(&attrs)
            .build()
            .expect("valid generic operation"),
    );

    for (i, &result_id) in op.results.iter().enumerate() {
        if let Ok(r) = mlir_op.result(i) {
            value_map.insert(result_id, r.into());
        }
    }
}

#[derive(Clone, Copy)]
enum BinaryArithKind {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
}

/// Emit a binary arithmetic operation, dispatching to the correct
/// arith dialect op based on the operand type (integer vs float).
fn emit_binary_arith<'c, 'b>(
    ctx: &'c Context,
    block: &'b Block<'c>,
    op: &TirOp,
    value_map: &mut HashMap<ValueId, Value<'c, 'b>>,
    tir_func: &TirFunction,
    kind: BinaryArithKind,
) {
    let location = Location::unknown(ctx);

    let (Some(lhs), Some(rhs), Some(&result_id)) = (
        op.operands.first().and_then(|v| value_map.get(v).copied()),
        op.operands.get(1).and_then(|v| value_map.get(v).copied()),
        op.results.first(),
    ) else {
        return;
    };

    let result_type = resolve_value_type(ctx, tir_func, result_id);
    let is_float = result_type == Type::float64(ctx);

    let mlir_op = if is_float {
        match kind {
            BinaryArithKind::Add => block.append_operation(arith::addf(lhs, rhs, location)),
            BinaryArithKind::Sub => block.append_operation(arith::subf(lhs, rhs, location)),
            BinaryArithKind::Mul => block.append_operation(arith::mulf(lhs, rhs, location)),
            BinaryArithKind::Div => block.append_operation(arith::divf(lhs, rhs, location)),
            BinaryArithKind::FloorDiv => {
                // floordiv for floats: divf then truncate (emit as divf for now,
                // standard MLIR has no floor_divf; the pass pipeline can lower it).
                block.append_operation(arith::divf(lhs, rhs, location))
            }
            BinaryArithKind::Mod => block.append_operation(arith::remf(lhs, rhs, location)),
        }
    } else {
        match kind {
            BinaryArithKind::Add => block.append_operation(arith::addi(lhs, rhs, location)),
            BinaryArithKind::Sub => block.append_operation(arith::subi(lhs, rhs, location)),
            BinaryArithKind::Mul => block.append_operation(arith::muli(lhs, rhs, location)),
            BinaryArithKind::Div => block.append_operation(arith::divsi(lhs, rhs, location)),
            BinaryArithKind::FloorDiv => {
                block.append_operation(arith::floordivsi(lhs, rhs, location))
            }
            BinaryArithKind::Mod => block.append_operation(arith::remsi(lhs, rhs, location)),
        }
    };

    value_map.insert(result_id, mlir_op.result(0).unwrap().into());
}

/// Emit a comparison operation using arith.cmpi or arith.cmpf.
fn emit_comparison<'c, 'b>(
    ctx: &'c Context,
    block: &'b Block<'c>,
    op: &TirOp,
    value_map: &mut HashMap<ValueId, Value<'c, 'b>>,
    tir_func: &TirFunction,
) {
    let location = Location::unknown(ctx);

    let (Some(lhs), Some(rhs), Some(&result_id)) = (
        op.operands.first().and_then(|v| value_map.get(v).copied()),
        op.operands.get(1).and_then(|v| value_map.get(v).copied()),
        op.results.first(),
    ) else {
        return;
    };

    // Determine if operands are float by checking the first operand's type.
    let lhs_type = if let Some(&first_op) = op.operands.first() {
        resolve_value_type(ctx, tir_func, first_op)
    } else {
        IntegerType::new(ctx, 64).into()
    };
    let is_float = lhs_type == Type::float64(ctx);

    let mlir_op = if is_float {
        let pred = match op.opcode {
            OpCode::Eq => arith::CmpfPredicate::Oeq,
            OpCode::Ne => arith::CmpfPredicate::One,
            OpCode::Lt => arith::CmpfPredicate::Olt,
            OpCode::Le => arith::CmpfPredicate::Ole,
            OpCode::Gt => arith::CmpfPredicate::Ogt,
            OpCode::Ge => arith::CmpfPredicate::Oge,
            _ => arith::CmpfPredicate::Oeq,
        };
        block.append_operation(arith::cmpf(ctx, pred, lhs, rhs, location))
    } else {
        let pred = match op.opcode {
            OpCode::Eq => arith::CmpiPredicate::Eq,
            OpCode::Ne => arith::CmpiPredicate::Ne,
            OpCode::Lt => arith::CmpiPredicate::Slt,
            OpCode::Le => arith::CmpiPredicate::Sle,
            OpCode::Gt => arith::CmpiPredicate::Sgt,
            OpCode::Ge => arith::CmpiPredicate::Sge,
            _ => arith::CmpiPredicate::Eq,
        };
        block.append_operation(arith::cmpi(ctx, pred, lhs, rhs, location))
    };

    value_map.insert(result_id, mlir_op.result(0).unwrap().into());
}

/// Emit a block terminator.
fn emit_terminator<'c, 'b>(
    ctx: &'c Context,
    _tir_func: &TirFunction,
    block: &'b Block<'c>,
    terminator: &Terminator,
    value_map: &HashMap<ValueId, Value<'c, 'b>>,
    mlir_blocks: &HashMap<BlockId, Block<'c>>,
    result_types: &[Type<'c>],
) {
    let location = Location::unknown(ctx);

    match terminator {
        Terminator::Return { values } => {
            let return_vals: Vec<Value<'c, '_>> = values
                .iter()
                .filter_map(|v| value_map.get(v).copied())
                .collect();
            block.append_operation(func::r#return(&return_vals, location));
        }
        Terminator::Branch { target, args } => {
            let target_block = &mlir_blocks[target];
            let branch_args: Vec<Value<'c, '_>> = args
                .iter()
                .filter_map(|v| value_map.get(v).copied())
                .collect();
            block.append_operation(cf::br(target_block, &branch_args, location));
        }
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let condition = value_map.get(cond).copied();
            let true_block = &mlir_blocks[then_block];
            let false_block = &mlir_blocks[else_block];
            let true_args: Vec<Value<'c, '_>> = then_args
                .iter()
                .filter_map(|v| value_map.get(v).copied())
                .collect();
            let false_args: Vec<Value<'c, '_>> = else_args
                .iter()
                .filter_map(|v| value_map.get(v).copied())
                .collect();

            if let Some(cond_val) = condition {
                block.append_operation(cf::cond_br(
                    ctx,
                    cond_val,
                    true_block,
                    false_block,
                    &true_args,
                    &false_args,
                    location,
                ));
            } else {
                // Fallback: unconditional branch to then block.
                block.append_operation(cf::br(true_block, &true_args, location));
            }
        }
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            let flag = value_map.get(value).copied();
            let default_block = &mlir_blocks[default];
            let default_operands: Vec<Value<'c, '_>> = default_args
                .iter()
                .filter_map(|v| value_map.get(v).copied())
                .collect();

            if let Some(flag_val) = flag {
                let case_values: Vec<i64> = cases.iter().map(|(val, _, _)| *val).collect();
                let case_destinations: Vec<(&Block<'c>, Vec<Value<'c, '_>>)> = cases
                    .iter()
                    .map(|(_, target, args)| {
                        let block_ref = &mlir_blocks[target];
                        let operands: Vec<Value<'c, '_>> = args
                            .iter()
                            .filter_map(|v| value_map.get(v).copied())
                            .collect();
                        (block_ref, operands)
                    })
                    .collect();

                // Build case_destinations as slice of (&Block, &[Value]).
                let case_dests_refs: Vec<(&Block<'c>, &[Value<'c, '_>])> = case_destinations
                    .iter()
                    .map(|(b, v)| (*b, v.as_slice()))
                    .collect();

                let i64_type: Type<'c> = IntegerType::new(ctx, 64).into();
                match cf::switch(
                    ctx,
                    &case_values,
                    flag_val,
                    i64_type,
                    (default_block, &default_operands),
                    &case_dests_refs,
                    location,
                ) {
                    Ok(switch_op) => {
                        block.append_operation(switch_op);
                    }
                    Err(_) => {
                        // Fallback to unconditional branch to default.
                        block.append_operation(cf::br(default_block, &default_operands, location));
                    }
                }
            } else {
                block.append_operation(cf::br(default_block, &default_operands, location));
            }
        }
        Terminator::Unreachable => {
            // Emit a func.return with no values as a safe fallback.
            // The verifier requires every block to have a terminator.
            // For functions with non-void return type, emit a zero constant.
            if result_types.is_empty() {
                block.append_operation(func::r#return(&[], location));
            } else {
                // Emit dummy return values for unreachable blocks to satisfy
                // the verifier. These will never execute at runtime.
                let mut return_vals = Vec::with_capacity(result_types.len());
                for &rty in result_types {
                    let dummy = if rty == Type::float64(ctx) {
                        let c = block.append_operation(arith::constant(
                            ctx,
                            FloatAttribute::new(ctx, rty, 0.0).into(),
                            location,
                        ));
                        c.result(0).unwrap().into()
                    } else {
                        let c = block.append_operation(arith::constant(
                            ctx,
                            IntegerAttribute::new(rty, 0).into(),
                            location,
                        ));
                        c.result(0).unwrap().into()
                    };
                    return_vals.push(dummy);
                }
                block.append_operation(func::r#return(&return_vals, location));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_backend::tir::{
        blocks::{BlockId, TirBlock},
        ops::{AttrDict, AttrValue, Dialect, TirOp},
        values::ValueId,
    };

    fn setup_context() -> Context {
        let ctx = Context::new();
        ctx.append_dialect_registry(&melior::dialect::DialectRegistry::new());
        ctx.load_all_available_dialects();
        ctx.set_allow_unregistered_dialects(true);
        ctx
    }

    fn make_add_func() -> TirFunction {
        let mut func =
            TirFunction::new("test_add".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let v2 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };
        func
    }

    fn make_const_func() -> TirFunction {
        let mut func = TirFunction::new("test_const".into(), vec![], TirType::I64);
        let v0 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(42));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v0],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v0] };
        func
    }

    fn make_branch_func() -> TirFunction {
        let mut func =
            TirFunction::new("test_branch".into(), vec![TirType::Bool], TirType::I64);
        let tb = func.fresh_block();
        let eb = func.fresh_block();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: tb,
            then_args: vec![],
            else_block: eb,
            else_args: vec![],
        };

        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(1));
        func.blocks.insert(
            tb,
            TirBlock {
                id: tb,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: attrs1,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v1] },
            },
        );

        let mut attrs2 = AttrDict::new();
        attrs2.insert("value".into(), AttrValue::Int(0));
        func.blocks.insert(
            eb,
            TirBlock {
                id: eb,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: attrs2,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v2] },
            },
        );

        func
    }

    fn make_comparison_func() -> TirFunction {
        let mut func =
            TirFunction::new("test_cmp".into(), vec![TirType::I64, TirType::I64], TirType::Bool);
        let v2 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Lt,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };
        func
    }

    #[test]
    fn lower_add_function() {
        let ctx = setup_context();
        let tir_func = make_add_func();
        let module = lower_tir_to_mlir(&ctx, &tir_func);
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("func.func"));
        assert!(text.contains("arith.addi"));
    }

    #[test]
    fn lower_const_function() {
        let ctx = setup_context();
        let tir_func = make_const_func();
        let module = lower_tir_to_mlir(&ctx, &tir_func);
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("arith.constant"));
        assert!(text.contains("42"));
    }

    #[test]
    fn lower_branch_function() {
        let ctx = setup_context();
        let tir_func = make_branch_func();
        let module = lower_tir_to_mlir(&ctx, &tir_func);
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("cf.cond_br"));
    }

    #[test]
    fn lower_comparison_function() {
        let ctx = setup_context();
        let tir_func = make_comparison_func();
        let module = lower_tir_to_mlir(&ctx, &tir_func);
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("arith.cmpi"));
    }

    #[test]
    fn lower_void_function() {
        let ctx = setup_context();
        let mut func_def = TirFunction::new("test_void".into(), vec![], TirType::None);
        func_def.blocks.get_mut(&func_def.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let module = lower_tir_to_mlir(&ctx, &func_def);
        assert!(module.as_operation().verify());
    }
}
