use super::*;

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

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeRcAuthority {
    NativeValueTracking,
    TirDropInsertion,
}

#[cfg(feature = "native-backend")]
impl NativeRcAuthority {
    pub(crate) fn from_drop_inserted(drop_inserted: bool) -> Self {
        if drop_inserted {
            Self::TirDropInsertion
        } else {
            Self::NativeValueTracking
        }
    }

    pub(crate) fn native_value_tracking_enabled(self) -> bool {
        matches!(self, Self::NativeValueTracking)
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

/// Select dead names from one block's candidate inventory. This helper does
/// not own references or remember releases: executable NativeCleanupRoots
/// tokens are the only native release authority across branches and loops.
#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_candidates(
    rc_authority: NativeRcAuthority,
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<String> {
    if !rc_authority.native_value_tracking_enabled() {
        names.clear();
        return Vec::new();
    }
    let mut cleanup = Vec::new();
    names.retain(|name| {
        if skip != Some(name.as_str())
            && last_use.get(name).copied().unwrap_or(usize::MAX) <= op_idx
        {
            cleanup.push(name.clone());
            false
        } else {
            true
        }
    });
    cleanup
}

// ---------------------------------------------------------------------------
// RC coalescing: eliminate redundant inc_ref / dec_ref pairs.
// ---------------------------------------------------------------------------
