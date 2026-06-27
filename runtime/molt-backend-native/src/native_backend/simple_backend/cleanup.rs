use super::*;

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(in crate::native_backend::simple_backend) fn collect_cleanup_tracked(
    names: &[String],
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<String> {
    names
        .iter()
        .filter(|name| skip != Some(name.as_str()))
        .filter(|name| last_use.get(*name).copied().unwrap_or(op_idx) <= op_idx)
        .cloned()
        .collect()
}

#[cfg(feature = "native-backend")]
pub(crate) fn extend_unique_tracked(dst: &mut Vec<String>, src: Vec<String>) {
    if src.is_empty() {
        return;
    }
    if dst.is_empty() {
        dst.extend(src);
        return;
    }
    // Dedup by `name` so multi-predecessor merges don't create double-decref hazards.
    let mut seen: BTreeSet<String> = dst.iter().cloned().collect();
    for name in src {
        if seen.insert(name.clone()) {
            dst.push(name);
        }
    }
}

/// Propagate tracked objects to ALL branch target blocks.
/// Prevents use-after-free when exception handlers access freed objects.
#[cfg(feature = "native-backend")]
pub(crate) fn propagate_tracked_to_branches(
    block_tracked: &mut BTreeMap<cranelift_codegen::ir::Block, Vec<String>>,
    targets: &[cranelift_codegen::ir::Block],
    carry: Vec<String>,
) {
    if carry.is_empty() || targets.is_empty() {
        return;
    }
    if targets.len() == 1 {
        extend_unique_tracked(block_tracked.entry(targets[0]).or_default(), carry);
        return;
    }
    let last_idx = targets.len() - 1;
    for (i, &target) in targets.iter().enumerate() {
        if i == last_idx {
            extend_unique_tracked(block_tracked.entry(target).or_default(), carry);
            return;
        }
        extend_unique_tracked(block_tracked.entry(target).or_default(), carry.clone());
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_tracked_dedup(
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    already_decrefed: Option<&mut BTreeSet<String>>,
) -> Vec<String> {
    drain_cleanup_tracked_dedup_with_budget(
        names,
        last_use,
        alias_roots,
        op_idx,
        skip,
        already_decrefed,
        None,
    )
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_tracked_dedup_with_authority(
    native_rc_tracking_enabled: bool,
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    already_decrefed: Option<&mut BTreeSet<String>>,
) -> Vec<String> {
    if !native_rc_tracking_enabled {
        names.clear();
        return Vec::new();
    }
    drain_cleanup_tracked_dedup(names, last_use, alias_roots, op_idx, skip, already_decrefed)
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_tracked_dedup_with_budget(
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    mut already_decrefed: Option<&mut BTreeSet<String>>,
    mut retain_release_budget: Option<&mut BTreeMap<String, usize>>,
) -> Vec<String> {
    let mut cleanup = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        let cleanup_key = alias_roots
            .get(name)
            .map(String::as_str)
            .unwrap_or(name.as_str());
        if let Some(ref mut set) = already_decrefed
            && set.contains(cleanup_key)
        {
            let budget_allows = if let Some(ref mut budget) = retain_release_budget {
                if let Some(extra) = budget.get_mut(cleanup_key) {
                    if *extra > 0 {
                        *extra -= 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !budget_allows {
                return false;
            }
        }
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            if let Some(ref mut set) = already_decrefed
                && !set.contains(cleanup_key)
            {
                set.insert(cleanup_key.to_string());
            }
            cleanup.push(name.clone());
            return false;
        }
        true
    });
    cleanup
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_entry_tracked_with_authority(
    native_rc_tracking_enabled: bool,
    names: &mut Vec<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<Value> {
    if !native_rc_tracking_enabled {
        for name in names.drain(..) {
            entry_vars.remove(&name);
        }
        return Vec::new();
    }
    drain_cleanup_entry_tracked(
        names,
        entry_vars,
        last_use,
        alias_roots,
        already_decrefed,
        op_idx,
        skip,
    )
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_entry_tracked(
    names: &mut Vec<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<Value> {
    let mut cleanup = Vec::new();
    let mut to_remove = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        // If not in last_use, default to MAX (keep alive) - NOT op_idx.
        // Using op_idx as default causes premature cleanup of variables
        // that are used later but not yet tracked in last_use.
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            let cleanup_key = alias_roots
                .get(name)
                .map(String::as_str)
                .unwrap_or(name.as_str());
            if already_decrefed.contains(cleanup_key) {
                to_remove.push(name.clone());
                return false;
            }
            if let Some(val) = entry_vars.get(name) {
                cleanup.push(*val);
            }
            already_decrefed.insert(cleanup_key.to_string());
            // Mark for removal from entry_vars so no other cleanup path
            // (exception handler, finalize block) can double dec-ref.
            to_remove.push(name.clone());
            return false;
        }
        true
    });
    for name in to_remove {
        entry_vars.remove(&name);
    }
    cleanup
}

// ---------------------------------------------------------------------------
// RC coalescing: eliminate redundant inc_ref / dec_ref pairs.
// ---------------------------------------------------------------------------
