use crate::tir::analysis::{AnalysisManager, LoopForest};
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_vectorize_facts_table;
use crate::tir::ops::AttrValue;
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;

use super::super::PassStats;
use super::analysis::{VectorizationInfo, analyse_loop};

/// Analyse all loops in a TIR function for vectorization potential.
///
/// Adds `"vectorize" = AttrValue::Bool(true)` and optionally
/// `"reduction" = AttrValue::Str("sum"|…)` to the `ForIter`/`ScfFor` op
/// in each vectorizable loop-header block.
///
/// Returns [`PassStats`] with `values_changed` set to the number of loops
/// annotated.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, tti: &TargetInfo) -> PassStats {
    let mut stats = PassStats {
        name: "vectorize",
        ..Default::default()
    };

    let loop_forest = am.get::<LoopForest>(func).clone();

    // For each loop, analyse and potentially annotate.
    // We collect (header, info) pairs first to avoid borrowing `func` mutably
    // while reading from it.
    let analyses: Vec<(BlockId, VectorizationInfo)> = loop_forest
        .headers
        .iter()
        .filter_map(|&header| {
            loop_forest
                .bodies
                .get(&header)
                .map(|body| (header, analyse_loop(func, body)))
        })
        .collect();

    for (header, info) in analyses {
        if !info.vectorizable {
            continue;
        }

        let block = match func.blocks.get_mut(&header) {
            Some(b) => b,
            None => continue,
        };

        // Find the first ForIter / ScfFor op in the header and annotate it.
        // If no such op exists, annotate the first arithmetic op we find.
        let target_op = block
            .ops
            .iter_mut()
            .find(|op| opcode_vectorize_facts_table(op.opcode).annotation_target);

        let op = match target_op {
            Some(o) => o,
            None => {
                // Fallback: annotate whatever is there (e.g. an Add for synthetic loops).
                match block.ops.first_mut() {
                    Some(o) => o,
                    None => continue,
                }
            }
        };

        op.attrs.insert("vectorize".into(), AttrValue::Bool(true));
        stats.values_changed += 1;

        if let Some(red) = info.reduction_op {
            op.attrs
                .insert("reduction".into(), AttrValue::Str(red.as_str().into()));
        }

        // Mojo/GCC 15 auto-vectorization: emit element type and SIMD width
        // hints so the LLVM backend can select the correct vector intrinsic
        // width. For I64 / F64 elements, typical SIMD widths are:
        //   - SSE2/NEON: 2 lanes (128-bit)
        //   - AVX2:      4 lanes (256-bit)
        //   - AVX-512:   8 lanes (512-bit)
        // We emit the conservative width (2) as the minimum; the backend
        // can widen based on target features. `i64` and `f64` share the
        // same lane width, so the lane count is identical for promoted
        // mixed-type loops and uniform `f64` loops.
        if let Some(ref elem_ty) = info.element_type {
            let ty_str = match elem_ty {
                TirType::I64 => "i64",
                TirType::F64 => "f64",
                // `analyse_loop` only ever sets element_type to I64 or F64
                // (Bool collapses into the I64 lane category). We still
                // surface a defensive default rather than panicking so
                // future numeric tower extensions degrade gracefully.
                _ => "unknown",
            };
            op.attrs
                .insert("element_type".into(), AttrValue::Str(ty_str.into()));
            // Cost model: SIMD lane count for this element type. The baseline
            // `native_release_fast` cost model returns 2 (128-bit minimum),
            // reproducing the prior hardcoded width exactly; a target-aware
            // cost model widens it from host SIMD caps (AVX2 → 4, AVX-512F → 8).
            let simd_width: i64 = tti.vector_width(elem_ty) as i64;
            op.attrs
                .insert("simd_width".into(), AttrValue::Int(simd_width));
        }

        // Mixed-type promotion hint: when the loop body mixed integer-shaped
        // and floating-point values, the analysis chose F64 lanes and the
        // backend must insert lane-wise `sitofp` on the integer-typed
        // operand loads. We surface this as an explicit attr so the LIR
        // lowering does not need to re-derive it.
        if info.promoted {
            op.attrs.insert("promoted".into(), AttrValue::Bool(true));
        }
    }

    stats
}
