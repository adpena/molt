use super::*;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn next_check_exception_target(
    ops: &[OpIR],
    op_idx: usize,
) -> Option<i64> {
    ops.iter()
        .skip(op_idx + 1)
        .find(|op| op.kind == "check_exception")
        .and_then(|op| op.value)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn remove_tracked_name(
    tracked: &mut Vec<String>,
    name: &str,
) {
    tracked.retain(|tracked_name| tracked_name != name);
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn is_join_slot_name(name: &str) -> bool {
    name.starts_with("_bb") && name.contains("_arg")
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn is_compiler_value_temp_name(
    name: &str,
) -> bool {
    name.strip_prefix("_v")
        .or_else(|| name.strip_prefix('v'))
        .is_some_and(|suffix| suffix.as_bytes().first().is_some_and(u8::is_ascii_digit))
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn is_persistent_local_slot_name(
    name: &str,
) -> bool {
    is_join_slot_name(name) || !is_compiler_value_temp_name(name)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn remove_tracked_alias_group(
    tracked: &mut Vec<String>,
    alias_roots: &BTreeMap<String, String>,
    root: &str,
) {
    tracked.retain(|name| alias_roots.get(name).map(String::as_str) != Some(root));
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn alias_root_name<'a>(
    alias_roots: &'a BTreeMap<String, String>,
    name: &'a str,
) -> &'a str {
    alias_roots.get(name).map(String::as_str).unwrap_or(name)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn cleanup_roots_for_names(
    alias_roots: &BTreeMap<String, String>,
    names: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    names
        .into_iter()
        .map(|name| alias_root_name(alias_roots, &name).to_string())
        .collect()
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn scrub_tracked_roots(
    roots: &BTreeSet<String>,
    alias_roots: &BTreeMap<String, String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    if roots.is_empty() {
        return;
    }
    tracked_obj_vars.retain(|n: &String| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    tracked_vars.retain(|n: &String| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    tracked_obj_vars_set.retain(|n| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    tracked_vars_set.retain(|n| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    entry_vars.retain(|name, _| !roots.contains(alias_root_name(alias_roots, name)));
    for tracked_list in block_tracked_obj.values_mut() {
        tracked_list.retain(|name| !roots.contains(alias_root_name(alias_roots, name.as_str())));
    }
    for tracked_list in block_tracked_ptr.values_mut() {
        tracked_list.retain(|name| !roots.contains(alias_root_name(alias_roots, name.as_str())));
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn mark_cleanup_root_once(
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    name: &str,
) -> bool {
    already_decrefed.insert(alias_root_name(alias_roots, name).to_string())
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn cleanup_name_excluded(
    name: &str,
    protected_names: Option<&BTreeSet<String>>,
    param_name_set: &BTreeSet<&str>,
    representation_plan: &ScalarRepresentationPlan,
) -> bool {
    protected_names.is_some_and(|protected| protected.contains(name))
        || param_name_set.contains(name)
        || representation_plan.is_raw_int_carrier_name(name)
        || representation_plan.is_float_unboxed(name)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn protect_cleanup_names(
    carry: &mut Vec<String>,
    cleanup: Vec<String>,
    protected: &BTreeSet<&str>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
) -> Vec<String> {
    if protected.is_empty() {
        return cleanup;
    }
    let mut preserved = Vec::new();
    let mut actual = Vec::new();
    for name in cleanup {
        if protected.contains(name.as_str()) {
            already_decrefed.remove(alias_root_name(alias_roots, &name));
            preserved.push(name);
        } else {
            actual.push(name);
        }
    }
    crate::extend_unique_tracked(carry, preserved);
    actual
}

/// Release dead block-tracked heap temporaries of the current block at a
/// generator/async `_poll` suspend boundary (`state_yield` / `state_transition`
/// / `chan_*_yield`), immediately before the jump to the master return block.
///
/// A `_poll` returns to its caller on every yield/await and is re-entered on the
/// next resume, so each suspend is the per-iteration scope exit for any heap
/// temporary that is dead before it.  Without this drain those temporaries —
/// chiefly the `(value, done)` pair tuple emitted right before each
/// `state_yield` — are re-allocated and orphaned on every resume, producing an
/// unbounded leak that delegation (`yield from` / manual for-yield) multiplies
/// by the chain depth.
///
/// Only names whose `last_use <= op_idx` are released (the
/// `drain_cleanup_tracked_dedup` gate); loop-carried values keep their
/// func_end-extended `last_use` and therefore survive the suspend.  This is the
/// suspend-boundary twin of the function-return drain in the `ret` handler,
/// restricted to the per-iteration temporaries identified by
/// `stateful_per_iter_temps`.
///
/// Free function (not a method): a live `FunctionBuilder` holds `&mut
/// self.ctx.func`, so a `&mut self` method taking the builder would double-borrow
/// `self` — the same reason the surrounding codegen routes through free helpers.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn drain_dead_block_temps_for_suspend(
    rc_authority: NativeRcAuthority,
    builder: &mut FunctionBuilder,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    entry_vars: &BTreeMap<String, Value>,
    vars: &BTreeMap<String, Variable>,
    local_dec_ref_obj: FuncRef,
    op_idx: usize,
) {
    let Some(block) = builder.current_block() else {
        return;
    };
    for tracked in [block_tracked_obj, block_tracked_ptr] {
        let Some(names) = tracked.get_mut(&block) else {
            continue;
        };
        let cleanup = drain_cleanup_tracked_dedup_with_authority(
            rc_authority,
            names,
            last_use,
            alias_roots,
            op_idx,
            None,
            Some(already_decrefed),
        );
        for name in cleanup {
            // Prefer the definition-time Value (entry_vars); fall back to the
            // current SSA value of the slot — identical to the `ret` cleanup
            // path (`resolve_cleanup_value`).  For a loop-body temporary the
            // current value is the freshly-defined object, which is exactly
            // what must be released at the suspend.  obj- and ptr-tracked
            // names both release through molt_dec_ref_obj (NaN-box aware).
            let Some(val) = resolve_cleanup_value(builder, vars, entry_vars, &name) else {
                continue;
            };
            builder.ins().call(local_dec_ref_obj, &[val]);
        }
    }
}
