use crate::SimpleIR;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Dead import elimination
//
// Removes `import` and `import_from` ops whose loaded module/name is never
// referenced by any subsequent op in the same function. This prevents pulling
// in entire stdlib modules for imports that the user code never actually uses.
// ---------------------------------------------------------------------------

/// Eliminate imports whose results are never consumed.
pub fn eliminate_dead_imports(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_IMPORT_ELIM").is_ok() {
        return;
    }

    let trace = std::env::var("MOLT_DEBUG_DEAD_IMPORT_ELIM").is_ok();
    let mut total_removed = 0usize;

    for func in &mut ir.functions {
        // Build the set of all consumed variable names.
        let mut consumed: HashSet<String> = HashSet::new();
        for op in &func.ops {
            if let Some(args) = &op.args {
                for arg in args {
                    consumed.insert(arg.clone());
                }
            }
        }

        let before = func.ops.len();

        func.ops.retain(|op| {
            // Only target import ops.
            if !matches!(op.kind.as_str(), "import_name" | "import_from") {
                return true;
            }

            // If the import result is consumed, keep it.
            if let Some(var) = &op.var {
                if consumed.contains(var) {
                    return true;
                }
                // The import result is never referenced → dead import.
                return false;
            }

            // No result var — keep conservatively.
            true
        });

        let removed = before - func.ops.len();
        total_removed += removed;
    }

    if trace && total_removed > 0 {
        eprintln!("dead-import-elim: removed {total_removed} dead imports across all functions");
    }
}
