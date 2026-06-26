use crate::{FunctionIR, OpIR};
use std::collections::HashSet;

/// Eliminate `check_exception` ops that follow operations known to never
/// raise exceptions. This reduces branch overhead in tight inner loops
/// (e.g., fib: 10 checks/call -> fewer).
///
/// Safe-to-elide predecessors: inc_ref, dec_ref, dec_ref_obj, const_int,
/// const_float, const_bool, const_none, nop, line.
/// Eliminate UnboundLocalError check sequences from the SimpleIR.
///
/// The frontend emits a `missing` + `is(var, missing)` + `br_if` +
/// `raise UnboundLocalError` guard for every local variable access.
/// In type-annotated functions and most generated code, variables are
/// always initialized before use, making these checks pure dead weight.
///
/// Each check sequence is ~11 ops and involves two function calls
/// (`molt_missing`, `molt_is`). In a tight inner loop like mandelbrot
/// with 12 variable accesses, this adds ~132 ops + 24 function calls
/// per iteration on top of the ~12 actual computation ops.
///
/// The pattern matched (with optional nop gaps):
///
/// ```text
/// [missing]       out=M
/// [is]            out=R  args=[V, M]
/// [jump]          val=L1
/// [label]         val=L1
/// [br_if]         args=[R]  val=L_raise
/// [jump]          val=L_ok
/// [label]         val=L_raise
/// [tuple_new]     sval="cannot access local variable ..."
/// [exception_new] / [exception_new_builtin] for "UnboundLocalError"
/// [raise]
/// [label]         val=L_ok
/// ```
///
/// This pass removes the entire sequence and any preceding nop,
/// leaving only the final continuation label intact.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn eliminate_unbound_local_checks(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_UNBOUND_ELIM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len < 11 {
        return;
    }

    // Collect output names of all `missing` ops for fast lookup.
    let missing_outputs: HashSet<&str> = ops
        .iter()
        .filter(|op| op.kind == "missing")
        .filter_map(|op| op.out.as_deref())
        .collect();

    if missing_outputs.is_empty() {
        return;
    }

    // Pre-build a set of const_str names whose value is "UnboundLocalError"
    // to avoid rescanning the entire ops array for every match.
    let unbound_error_names: HashSet<&str> = ops
        .iter()
        .filter(|op| op.kind == "const_str" && op.s_value.as_deref() == Some("UnboundLocalError"))
        .filter_map(|op| op.out.as_deref())
        .collect();
    let is_unbound_exception_new = |op: &OpIR| -> bool {
        if matches!(
            op.kind.as_str(),
            "exception_new_builtin" | "exception_new_builtin_empty" | "exception_new_builtin_one"
        ) {
            return op.s_value.as_deref() == Some("UnboundLocalError");
        }
        if op.kind != "exception_new" {
            return false;
        }
        op.args
            .as_ref()
            .is_some_and(|args| !args.is_empty() && unbound_error_names.contains(args[0].as_str()))
    };

    let mut remove = vec![false; len];
    let mut i = 0;
    while i + 9 < len {
        // Skip optional nop before the `is` op.
        let base = i;
        if ops[i].kind == "nop" {
            i += 1;
            if i + 9 >= len {
                break;
            }
        }

        // [0] is out=R args=[V, M]  — second arg must be a known missing sentinel
        if ops[i].kind != "is" {
            i = base + 1;
            continue;
        }
        let is_args = match ops[i].args.as_ref() {
            Some(args) if args.len() == 2 => args,
            _ => {
                i = base + 1;
                continue;
            }
        };
        if !missing_outputs.contains(is_args[1].as_str()) {
            i = base + 1;
            continue;
        }
        let is_out = match ops[i].out.as_deref() {
            Some(r) => r,
            None => {
                i = base + 1;
                continue;
            }
        };

        // ── Variant B: is → if → tuple_new → exception_new → raise → end_if ──
        // The frontend emits `if` directly (before TIR adds jump/label/br_if).
        let j1 = i + 1;
        if j1 < len && ops[j1].kind == "if" {
            let if_args = ops[j1].args.as_ref();
            let if_matches = if_args.is_some_and(|a| !a.is_empty() && a[0] == is_out);
            if if_matches {
                // Scan forward for tuple_new → exception_new → raise → end_if
                let mut k = j1 + 1;
                let mut found_tuple_new = false;
                let mut found_exc_new = false;
                let mut found_raise = false;
                let mut end_idx = 0usize;
                let max_scan = (j1 + 8).min(len);
                while k < max_scan {
                    match ops[k].kind.as_str() {
                        "tuple_new" if !found_tuple_new => found_tuple_new = true,
                        _ if found_tuple_new
                            && !found_exc_new
                            && is_unbound_exception_new(&ops[k]) =>
                        {
                            found_exc_new = true;
                        }
                        "raise" if found_exc_new && !found_raise => found_raise = true,
                        "end_if" | "else" if found_raise => {
                            end_idx = k;
                            break;
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if found_raise && end_idx > 0 {
                    // Match confirmed.  Remove the entire is → if → ... → raise → end_if
                    // sequence.  Both the `if` and `end_if`/`else` must be removed
                    // together to keep structured control flow consistent.
                    if base != i {
                        remove[base] = true;
                    }
                    for idx in i..=end_idx {
                        remove[idx] = true;
                    }
                    i = end_idx + 1;
                    continue;
                }
            }
            i = base + 1;
            continue;
        }

        // ── Variant A: is → jump → label → br_if → jump → label → tuple_new → exception_new → raise → label ──
        if j1 >= len || ops[j1].kind != "jump" {
            i = base + 1;
            continue;
        }

        // [2] label val=L1
        let j2 = j1 + 1;
        if j2 >= len || ops[j2].kind != "label" {
            i = base + 1;
            continue;
        }

        // [3] br_if args=[R] val=L_raise
        let j3 = j2 + 1;
        if j3 >= len || ops[j3].kind != "br_if" {
            i = base + 1;
            continue;
        }
        let brif_args = match ops[j3].args.as_ref() {
            Some(args) if !args.is_empty() => args,
            _ => {
                i = base + 1;
                continue;
            }
        };
        if brif_args[0] != is_out {
            i = base + 1;
            continue;
        }

        // [4] jump val=L_ok
        let j4 = j3 + 1;
        if j4 >= len || ops[j4].kind != "jump" {
            i = base + 1;
            continue;
        }

        // [5] label val=L_raise
        let j5 = j4 + 1;
        if j5 >= len || ops[j5].kind != "label" {
            i = base + 1;
            continue;
        }

        // [6] tuple_new (exception message)
        let j6 = j5 + 1;
        if j6 >= len || ops[j6].kind != "tuple_new" {
            i = base + 1;
            continue;
        }

        // [7] exception_new / exception_new_builtin with "UnboundLocalError"
        let j7 = j6 + 1;
        if j7 >= len || !is_unbound_exception_new(&ops[j7]) {
            i = base + 1;
            continue;
        }

        // [8] raise
        let j8 = j7 + 1;
        if j8 >= len || ops[j8].kind != "raise" {
            i = base + 1;
            continue;
        }

        // [9] label val=L_ok  (continuation)
        let j9 = j8 + 1;
        if j9 >= len || ops[j9].kind != "label" {
            i = base + 1;
            continue;
        }

        // Match confirmed. Mark the entire sequence for removal,
        // EXCEPT the final continuation label (j9) which other
        // code may jump to.
        if base != i {
            // We skipped a nop before the `is` op
            remove[base] = true;
        }
        for idx in i..=j8 {
            remove[idx] = true;
        }
        // Keep j9 (continuation label).

        i = j9 + 1;
    }

    // Also remove orphaned `missing` ops whose outputs are no longer
    // referenced after we stripped the `is` ops above.
    if remove.iter().any(|&r| r) {
        let surviving_args: HashSet<&str> = ops
            .iter()
            .enumerate()
            .filter(|&(idx, _)| !remove[idx])
            .flat_map(|(_, op)| {
                op.args
                    .as_ref()
                    .into_iter()
                    .flat_map(|a| a.iter().map(String::as_str))
            })
            .collect();
        for (idx, op) in ops.iter().enumerate() {
            if op.kind == "missing"
                && let Some(out) = op.out.as_deref()
                && !surviving_args.contains(out)
            {
                remove[idx] = true;
            }
        }
    }

    let count = remove.iter().filter(|&&r| r).count();
    if count > 0 {
        let mut new_ops = Vec::with_capacity(len - count);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if !remove[i] {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
    if std::env::var("MOLT_TRACE_UNBOUND_ELIM").is_ok() {
        let surviving_missing = func_ir.ops.iter().filter(|op| op.kind == "missing").count();
        if surviving_missing > 0 {
            eprintln!(
                "UNBOUND_ELIM: {} removed={} surviving_missing={}",
                func_ir.name, count, surviving_missing
            );
        }
    }
}
