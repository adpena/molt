#![no_main]
use libfuzzer_sys::fuzz_target;

use std::collections::HashMap;

use molt_backend::tir::blocks::{BlockId, LoopRole, TirBlock, Terminator};
use molt_backend::tir::function::TirFunction;
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_backend::tir::passes;
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::{ValueId, TirValue};

// --------------------------------------------------------------------------
// Fuzz target: build random TIR functions from structured fuzz data and run
// the full TIR optimization pipeline.  The goal is to find panics, infinite
// loops, or memory corruption in passes like DCE, SCCP, unboxing, escape
// analysis, strength reduction, and BCE.
// --------------------------------------------------------------------------

/// Small palette of opcodes that operate on 0-2 operands and produce 0-1
/// results, suitable for generating without needing complex invariants.
const OPCODES: &[OpCode] = &[
    OpCode::Add,
    OpCode::Sub,
    OpCode::Mul,
    OpCode::Neg,
    OpCode::Not,
    OpCode::Eq,
    OpCode::Lt,
    OpCode::BitAnd,
    OpCode::BitOr,
    OpCode::Shl,
    OpCode::ConstInt,
    OpCode::ConstFloat,
    OpCode::ConstBool,
    OpCode::ConstNone,
    OpCode::Copy,
    OpCode::IncRef,
    OpCode::DecRef,
    OpCode::BoxVal,
    OpCode::UnboxVal,
];

const TYPES: &[TirType] = &[
    TirType::I64,
    TirType::F64,
    TirType::Bool,
    TirType::None,
    TirType::DynBox,
];

/// Consume one byte from the front of `data`, advancing the slice.
fn eat(data: &mut &[u8]) -> Option<u8> {
    if data.is_empty() {
        None
    } else {
        let b = data[0];
        *data = &data[1..];
        Some(b)
    }
}

/// Build a TirFunction from raw fuzz bytes.  The function will have a
/// configurable number of blocks with random ops and terminators.
fn build_function(data: &mut &[u8]) -> Option<TirFunction> {
    // Number of params: 0..=3
    let num_params = (eat(data)? & 0x03) as usize;
    let mut param_types = Vec::with_capacity(num_params);
    for _ in 0..num_params {
        let ty_idx = (eat(data)? as usize) % TYPES.len();
        param_types.push(TYPES[ty_idx].clone());
    }
    let ret_idx = (eat(data)? as usize) % TYPES.len();
    let return_type = TYPES[ret_idx].clone();

    let mut func = TirFunction::new("fuzz_fn".to_string(), param_types, return_type.clone());

    // Number of extra blocks: 0..=7
    let num_extra_blocks = (eat(data)? & 0x07) as u32;
    let mut block_ids: Vec<BlockId> = vec![func.entry_block];

    for _ in 0..num_extra_blocks {
        let bid = BlockId(func.next_block);
        func.next_block += 1;

        // 0-2 block args
        let num_args = (eat(data).unwrap_or(0) & 0x03) as usize;
        let mut args = Vec::with_capacity(num_args);
        for _ in 0..num_args {
            let ty_idx = (eat(data).unwrap_or(0) as usize) % TYPES.len();
            let vid = ValueId(func.next_value);
            func.next_value += 1;
            args.push(TirValue {
                id: vid,
                ty: TYPES[ty_idx].clone(),
            });
        }

        let block = TirBlock {
            id: bid,
            args,
            ops: Vec::new(),
            terminator: Terminator::Unreachable,
        };
        func.blocks.insert(bid, block);
        block_ids.push(bid);
    }

    // Collect all defined ValueIds so ops can reference them.
    let mut all_values: Vec<ValueId> = Vec::new();
    for block in func.blocks.values() {
        for arg in &block.args {
            all_values.push(arg.id);
        }
    }

    // Populate ops in each block.
    for &bid in &block_ids {
        let num_ops = (eat(data).unwrap_or(0) & 0x0F) as usize; // 0..=15
        let mut ops = Vec::with_capacity(num_ops);

        for _ in 0..num_ops {
            let op_idx = (eat(data).unwrap_or(0) as usize) % OPCODES.len();
            let opcode = OPCODES[op_idx];

            // Determine operand count based on opcode.
            let num_operands = match opcode {
                OpCode::ConstInt | OpCode::ConstFloat | OpCode::ConstBool | OpCode::ConstNone => 0,
                OpCode::Neg | OpCode::Not | OpCode::Copy | OpCode::IncRef | OpCode::DecRef
                | OpCode::BoxVal | OpCode::UnboxVal => 1,
                _ => 2,
            };

            let mut operands = Vec::with_capacity(num_operands);
            for _ in 0..num_operands {
                if all_values.is_empty() {
                    break;
                }
                let idx = (eat(data).unwrap_or(0) as usize) % all_values.len();
                operands.push(all_values[idx]);
            }

            // Result: most ops produce one value.
            let has_result = !matches!(
                opcode,
                OpCode::IncRef | OpCode::DecRef
            );

            let results = if has_result {
                let vid = ValueId(func.next_value);
                func.next_value += 1;
                all_values.push(vid);
                vec![vid]
            } else {
                vec![]
            };

            // Attributes for const ops.
            let mut attrs: AttrDict = HashMap::new();
            match opcode {
                OpCode::ConstInt => {
                    let val = eat(data).unwrap_or(0) as i64 - 128;
                    attrs.insert("value".to_string(), AttrValue::Int(val));
                }
                OpCode::ConstFloat => {
                    let byte = eat(data).unwrap_or(0);
                    attrs.insert(
                        "value".to_string(),
                        AttrValue::Float(byte as f64 / 10.0),
                    );
                }
                OpCode::ConstBool => {
                    let b = (eat(data).unwrap_or(0) & 1) != 0;
                    attrs.insert("value".to_string(), AttrValue::Bool(b));
                }
                _ => {}
            }

            ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands,
                results,
                attrs,
                source_span: None,
            });
        }

        if let Some(block) = func.blocks.get_mut(&bid) {
            block.ops = ops;
        }
    }

    // Set terminators.  The entry block must eventually return. Other blocks
    // either branch to a random target or return.
    for i in 0..block_ids.len() {
        let bid = block_ids[i];
        let term_byte = eat(data).unwrap_or(0);

        let terminator = if i == 0 && block_ids.len() > 1 && (term_byte & 0x01) != 0 {
            // Branch to a random successor.
            let target_idx = (term_byte as usize >> 1) % (block_ids.len() - 1) + 1;
            Terminator::Branch {
                target: block_ids[target_idx],
                args: vec![],
            }
        } else if i > 0 && (term_byte & 0x02) != 0 && !all_values.is_empty() {
            // Conditional branch between two blocks.
            let cond_idx = (term_byte as usize >> 2) % all_values.len();
            let then_idx = if block_ids.len() > 1 {
                (term_byte as usize >> 4) % block_ids.len()
            } else {
                0
            };
            Terminator::CondBranch {
                cond: all_values[cond_idx],
                then_block: block_ids[then_idx],
                then_args: vec![],
                else_block: block_ids[0],
                else_args: vec![],
            }
        } else {
            // Return with zero values (safe default).
            Terminator::Return { values: vec![] }
        };

        if let Some(block) = func.blocks.get_mut(&bid) {
            block.terminator = terminator;
        }
    }

    // Mark some blocks as loop headers/ends for loop-sensitive passes.
    if block_ids.len() >= 3 {
        let role_byte = eat(data).unwrap_or(0);
        if (role_byte & 0x01) != 0 {
            func.loop_roles
                .insert(block_ids[1], LoopRole::LoopHeader);
            if block_ids.len() > 2 {
                func.loop_roles
                    .insert(block_ids[block_ids.len() - 1], LoopRole::LoopEnd);
            }
        }
    }

    Some(func)
}

fuzz_target!(|data: &[u8]| {
    let mut cursor = data;
    let Some(mut func) = build_function(&mut cursor) else {
        return;
    };

    // Run the full optimization pipeline.  We catch panics to distinguish
    // them from expected verification failures (which return empty stats).
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        passes::run_pipeline(&mut func)
    }));

    match result {
        Ok(stats) => {
            // Empty stats = verification failure, which is acceptable for
            // random input.  Non-empty = passes completed successfully.
            for s in &stats {
                // Sanity: pass names should be non-empty.
                assert!(
                    !s.name.is_empty(),
                    "optimization pass returned empty name"
                );
            }
        }
        Err(panic_info) => {
            // Re-panic so the fuzzer registers this as a crash.
            std::panic::resume_unwind(panic_info);
        }
    }
});
