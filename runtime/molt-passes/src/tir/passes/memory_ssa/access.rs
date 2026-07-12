use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::passes::alias_analysis::MemRegion;

// ===========================================================================
// MemVersion
// ===========================================================================

/// A memory access ordinal — unique per function, allocated sequentially.
/// Version `0` is the [`LIVE_ON_ENTRY`] def (all externally-visible memory
/// before the function's first op).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemVersion(pub u32);

/// The synthetic def representing all memory live before the function's first
/// op. Every reaching-def query bottoms out here when no in-function Def or Phi
/// dominates the use — the fail-closed floor.
pub const LIVE_ON_ENTRY: MemVersion = MemVersion(0);

// ===========================================================================
// MemAccess
// ===========================================================================

/// A node in the MemorySSA graph.
#[derive(Debug, Clone, PartialEq)]
pub enum MemAccess {
    /// A defining write/clobber. The op at `(block, op_idx)` defines memory
    /// version `ver`, consuming the preceding definition `def_ver` (the memory
    /// it "flows through" — the immediate dominating Def/Phi whose region it
    /// may-clobbers, needed for cross-block kill queries).
    Def {
        ver: MemVersion,
        /// The version this def flows through (its immediate memory dominator).
        def_ver: MemVersion,
        block: BlockId,
        op_idx: usize,
        /// The region this def clobbers, from [`AliasAnalysisResult::region_of`].
        region: MemRegion,
    },
    /// A memory use (a proven-pure load). The op at `(block, op_idx)` reads
    /// version `def_ver` — the most-recent dominating Def/Phi whose region may
    /// alias the load's region.
    Use {
        def_ver: MemVersion,
        block: BlockId,
        op_idx: usize,
        region: MemRegion,
    },
    /// A phi placed at a join point where multiple memory versions meet.
    Phi {
        ver: MemVersion,
        block: BlockId,
        /// `(predecessor BlockId, incoming MemVersion)` pairs.
        incoming: Vec<(BlockId, MemVersion)>,
    },
}

impl MemAccess {
    /// The version this access defines, if it is a Def or Phi (Uses define no
    /// new version).
    #[inline]
    pub fn defined_version(&self) -> Option<MemVersion> {
        match self {
            MemAccess::Def { ver, .. } | MemAccess::Phi { ver, .. } => Some(*ver),
            MemAccess::Use { .. } => None,
        }
    }

    /// The block this access lives in.
    #[inline]
    pub fn block(&self) -> BlockId {
        match self {
            MemAccess::Def { block, .. }
            | MemAccess::Use { block, .. }
            | MemAccess::Phi { block, .. } => *block,
        }
    }
}

// ===========================================================================
// MemorySsaResult
// ===========================================================================

/// The complete MemorySSA result for one function.
// `PartialEq` is required by the `MOLT_VERIFY_ANALYSIS` staleness self-check
// (pass_manager's cached-vs-fresh recompute comparison).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemorySsaResult {
    /// Every Def and Phi, keyed by the version it defines. (Uses define no
    /// version and are recorded only in `block_op_to_use_def`.)
    pub defs: HashMap<MemVersion, MemAccess>,
    /// `(block, op_idx)` → the [`MemVersion`] this op defines (Defs only).
    pub block_op_to_def: HashMap<(BlockId, usize), MemVersion>,
    /// `(block, op_idx)` → the [`MemVersion`] this op *reads* (Uses only).
    pub block_op_to_use_def: HashMap<(BlockId, usize), MemVersion>,
    /// Every memory use ([`MemAccess::Use`]), keyed by its `(block, op_idx)`
    /// position, carrying its region and reaching def. Consumers (MemGVN /
    /// LICM-of-loads) iterate this to find forwardable / hoistable loads.
    pub uses: HashMap<(BlockId, usize), MemAccess>,
    /// Per-block memory phi (the version the phi defines), when one was placed.
    pub block_phis: HashMap<BlockId, MemVersion>,
    /// The reaching def at the END of each block (the version that exits it).
    pub exit_def: HashMap<BlockId, MemVersion>,
    /// Next fresh version counter (the count of versions allocated, including
    /// [`LIVE_ON_ENTRY`]). Consumers that splice in new Defs allocate from here.
    pub next_version: u32,
}

impl MemorySsaResult {
    /// The memory version reaching a USE at `(block, op_idx)`: the most-recent
    /// def dominating that use whose region may-alias the load's region.
    /// `None` if the op at that position is not a tracked memory use.
    #[inline]
    pub fn reaching_def_for_use(&self, block: BlockId, op_idx: usize) -> Option<MemVersion> {
        self.block_op_to_use_def.get(&(block, op_idx)).copied()
    }

    /// The version a DEF at `(block, op_idx)` produces, if any.
    #[inline]
    pub fn def_at(&self, block: BlockId, op_idx: usize) -> Option<MemVersion> {
        self.block_op_to_def.get(&(block, op_idx)).copied()
    }

    /// For a [`MemAccess::Def`], the single Def/Phi it flows through (the memory
    /// it observes in the clobber graph). A Phi flows from itself (it *is* the
    /// merged version); a Use defines nothing and returns `None`.
    pub fn def_version_of(&self, ver: MemVersion) -> Option<MemVersion> {
        match self.defs.get(&ver)? {
            MemAccess::Def { def_ver, .. } => Some(*def_ver),
            MemAccess::Phi { ver, .. } => Some(*ver),
            MemAccess::Use { .. } => None,
        }
    }

    /// The [`MemAccess`] that defines `ver` (a Def or a Phi), if recorded.
    #[inline]
    pub fn access(&self, ver: MemVersion) -> Option<&MemAccess> {
        self.defs.get(&ver)
    }

    /// True if `store_ver` is exactly the reaching def of the load at
    /// `(load_block, load_op_idx)` — the single direct memory dependency used
    /// for store-to-load forwarding. False when the load's reaching def is a
    /// phi, a different store, or an intervening clobber.
    #[inline]
    pub fn is_direct_def_of_use(
        &self,
        store_ver: MemVersion,
        load_block: BlockId,
        load_op_idx: usize,
    ) -> bool {
        self.block_op_to_use_def
            .get(&(load_block, load_op_idx))
            .copied()
            == Some(store_ver)
    }
}
