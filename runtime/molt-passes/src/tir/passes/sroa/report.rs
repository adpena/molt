use crate::tir::function::TirFunction;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_report(
    report: bool,
    func: &TirFunction,
    raw_stack_allocs: usize,
    candidates: usize,
    promoted: usize,
    stores_removed: usize,
    diag: &[String],
) {
    if !report || raw_stack_allocs == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(diag.len() + 1);
    lines.push(format!(
        "[SROA] fn={} stack_allocs={raw_stack_allocs} candidates={candidates} \
         promoted={promoted} stores_removed={stores_removed}",
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
