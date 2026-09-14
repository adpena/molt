use crate::tir::function::TirFunction;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_report(
    report: bool,
    func: &TirFunction,
    fixed_layout_allocations: usize,
    candidates: usize,
    promoted: usize,
    ops_removed: usize,
    diag: &[String],
) {
    if !report || fixed_layout_allocations == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(diag.len() + 1);
    lines.push(format!(
        "[SROA] fn={} fixed_layout_allocations={fixed_layout_allocations} candidates={candidates} \
         promoted={promoted} ops_removed={ops_removed}",
        func.name
    ));
    lines.extend(diag.iter().cloned());
    for line in &lines {
        eprintln!("{line}");
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
        format!("sroa_report/{sanitized}.txt"),
        lines.join("\n") + "\n",
    );
}
