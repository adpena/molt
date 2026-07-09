use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;

use super::super::scev::ScevResult;
use super::ValueRangeResult;
/// Write the `MOLT_VRANGE_REPORT` per-function diagnostics: the loop headers with
/// their trip count + per-arg SCEV, then every proven global integer range
/// (ascending by value id). A no-op unless the report flag is set by the caller.
pub(super) fn emit_vrange_report(
    func: &TirFunction,
    scev: &ScevResult,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
    result: &ValueRangeResult,
) {
    let mut lines = vec![format!(
        "[VRANGE] fn={} headers={:?} loop_headers={:?}",
        func.name,
        scev.headers(),
        loop_bodies.keys().collect::<Vec<_>>()
    )];
    for h in scev.headers() {
        lines.push(format!("  header {:?} trip={:?}", h, scev.trip_count(*h)));
        if let Some(hb) = func.blocks.get(h) {
            for arg in &hb.args {
                lines.push(format!(
                    "    arg v{} scev={:?}",
                    arg.id.0,
                    scev.scev_of(arg.id)
                ));
            }
        }
    }
    let mut gr: Vec<_> = result.global_ranges().collect();
    gr.sort_by_key(|(v, _)| v.0);
    for (v, r) in gr {
        lines.push(format!("  v{} -> [{}, {}]", v.0, r.lo, r.hi));
    }
    let sanitized: String = func
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("vrange_report/{sanitized}.txt"),
        lines.join("\n") + "\n",
    );
}
