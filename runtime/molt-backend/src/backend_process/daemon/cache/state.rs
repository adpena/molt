use std::sync::Arc;

use molt_ir::FunctionIR;
use molt_ir::addressed_program::FunctionDigest;
use molt_passes::resident_cache::ResidentCache;

use super::super::origin::DaemonCacheEntry;
use super::super::protocol::DaemonCacheKey;

/// What the daemon keeps resident: compiled artifacts by their typed cache
/// identity, and admitted function bodies by their content digest (HF-516).
#[derive(PartialEq, Eq, Hash)]
enum ResidentKey {
    Artifact(Arc<DaemonCacheKey>),
    Function(FunctionDigest),
}

enum ResidentValue {
    Artifact(Arc<DaemonCacheEntry>),
    Function(Arc<FunctionIR>),
}

/// The shared compiler eviction authority. Artifacts and function bodies share
/// one budget and one recency order; keys and storage both count.
pub(crate) struct DaemonLru {
    resident: ResidentCache<ResidentKey, ResidentValue>,
}

/// Two words of `Arc` reference counts in front of each shared allocation.
const ARC_HEADER_BYTES: usize = 2 * std::mem::size_of::<usize>();

impl DaemonLru {
    pub(crate) fn new(max_bytes: Option<usize>) -> Self {
        Self {
            resident: ResidentCache::new(max_bytes),
        }
    }

    pub(crate) fn get_entry(&mut self, key: &Arc<DaemonCacheKey>) -> Option<Arc<DaemonCacheEntry>> {
        match self.resident.get(&ResidentKey::Artifact(Arc::clone(key)))? {
            ResidentValue::Artifact(bytes) => Some(Arc::clone(bytes)),
            ResidentValue::Function(_) => unreachable!("artifact keys hold artifacts"),
        }
    }

    pub(crate) fn insert(&mut self, key: Arc<DaemonCacheKey>, value: Arc<DaemonCacheEntry>) {
        let allocation_bytes = key.retained_bytes().saturating_add(value.retained_bytes());
        self.resident.insert(
            ResidentKey::Artifact(key),
            ResidentValue::Artifact(value),
            allocation_bytes,
        );
    }

    /// A lookup promotes the body, as a `FindMissingBlobs` probe extends the
    /// lifetime of the blobs it finds.
    pub(crate) fn get_function(&mut self, digest: FunctionDigest) -> Option<Arc<FunctionIR>> {
        match self.resident.get(&ResidentKey::Function(digest))? {
            ResidentValue::Function(function) => Some(Arc::clone(function)),
            ResidentValue::Artifact(_) => unreachable!("function keys hold functions"),
        }
    }

    /// Keep an admitted body. Its charge is the function's exact heap ownership
    /// plus its `Arc` header; one body larger than the budget is refused.
    pub(crate) fn insert_function(&mut self, digest: FunctionDigest, function: Arc<FunctionIR>) {
        let allocation_bytes = function.retained_bytes().saturating_add(ARC_HEADER_BYTES);
        self.resident.insert(
            ResidentKey::Function(digest),
            ResidentValue::Function(function),
            allocation_bytes,
        );
    }

    /// Entry count and charged retained ownership bytes, both kinds together.
    pub(crate) fn usage(&self) -> (usize, usize) {
        (self.resident.len(), self.resident.bytes())
    }

    /// Resident function bodies; a linear scan for health reports and tests.
    pub(crate) fn function_entries(&self) -> usize {
        self.resident
            .values()
            .filter(|value| matches!(value, ResidentValue::Function(_)))
            .count()
    }
}
