//! Bidirectional identity, static binding and foreign-wrapper transactions.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;

#[cfg(test)]
use super::publication::ManagedEntryRejection;
use super::publication::{is_publication_owner, observe_publication};

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct BridgeIdentity(pub(super) AbiHandle);

impl BridgeIdentity {
    #[inline]
    pub const fn as_handle(self) -> AbiHandle {
        self.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct MoltValueHandle(pub(super) AbiHandle);

impl MoltValueHandle {
    #[inline]
    pub const fn bits(self) -> AbiHandle {
        self.0
    }

    #[inline]
    pub(crate) fn decode(self) -> MoltObject {
        MoltObject::from_bits(self.0)
    }
}

/// Resolve canonical managed/scalar ABI views without interpreting arbitrary
/// pointer address bits as object values. Everything else is foreign.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum ResolvedPyObject {
    ManagedMolt(MoltValueHandle),
    Foreign,
}

#[inline]
pub(crate) fn resolve_pyobject(ptr: *mut PyObject) -> Option<ResolvedPyObject> {
    if ptr.is_null() {
        return None;
    }
    Some(match GLOBAL_BRIDGE.semantic_handle_for_pyobj(ptr).ok()? {
        Some(handle) => ResolvedPyObject::ManagedMolt(handle),
        None => {
            admit_foreign_pyobject(ptr)?;
            ResolvedPyObject::Foreign
        }
    })
}

/// Reference ownership never allocates a runtime identity. In particular an
/// adoption failure must not suppress INCREF/DECREF of the still-live original.
pub(crate) fn admit_reference(pointer: *mut PyObject) -> bool {
    if pointer.is_null() {
        return false;
    }
    if GLOBAL_BRIDGE.pyobj_to_handle(pointer).is_some() {
        return true;
    }
    let numeric = {
        let address = GLOBAL_BRIDGE.address_shard(pointer.addr()).lock();
        address.numeric_carriers.contains_key(&pointer.addr())
    };
    numeric || admit_foreign_pyobject(pointer).is_some()
}

/// Membership is queried only after managed identity misses, without holding
/// any bridge lock. The runtime releases its registry lock before we allocate
/// an error; source-buffer provenance cannot license a CPython layout cast.
fn admit_foreign_pyobject(ptr: *mut PyObject) -> Option<()> {
    if unsafe { (crate::hooks::hooks_or_stubs().private_c_heap_contains)(ptr.addr()) } == 0 {
        return Some(());
    }
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
        && !crate::api::errors::transfer_runtime_pending_to_current()
    {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                c"registered private C-heap storage has no CPython object protocol".as_ptr(),
            )
        };
    }
    None
}

#[inline]
pub(crate) fn resolved_molt_handle(ptr: *mut PyObject) -> Option<MoltValueHandle> {
    GLOBAL_BRIDGE.observed_handle_for_pyobj(ptr)
}

/// Observe a numeric/protocol operand without treating a failed managed-view
/// commit as foreign identity. The caller keeps the C object alive throughout
/// the observation and reports NULL arguments according to its API contract.
pub(crate) fn observe_pyobject(ptr: *mut PyObject) -> Option<ResolvedPyObject> {
    match resolve_pyobject(ptr)? {
        ResolvedPyObject::ManagedMolt(value) => GLOBAL_BRIDGE
            .prepare_runtime_value(value, RuntimeValueAccess::Observe)
            .map(ResolvedPyObject::ManagedMolt),
        ResolvedPyObject::Foreign => Some(ResolvedPyObject::Foreign),
    }
}

/// Mapping from MoltHandle bits → allocated PyObject header.
/// Entries live until the extension signals dealloc via Py_DECREF → 0.
///
/// Identity is recovered only from the bridge maps. No object-address or
/// adjacent-memory encoding participates in the ABI contract.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumericCarrierKind {
    Long { allocation_size: usize },
    Float,
    Complex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NumericCarrierRecord {
    pub bits: Option<AbiHandle>,
    pub kind: NumericCarrierKind,
}

/// Raw reverse identities are borrowed from another lifetime authority
/// (foreign wrappers and static bindings) and never own a runtime reference.
/// Runtime-backed ABI callables are canonical managed views instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RawBinding {
    pub(super) address: usize,
}

impl RawBinding {
    pub(super) fn borrowed(address: usize) -> Self {
        Self { address }
    }
}

/// One semantic crossing's runtime custody. Existing canonical values are
/// borrowed from the caller's C owner; foreign wrappers transfer one temporary
/// runtime reference, released only after consumers acquire their own edges.
pub(crate) struct RuntimeValue {
    bits: AbiHandle,
    owned: bool,
}

#[derive(Clone, Copy)]
pub(super) enum RuntimeValueAccess {
    Observe,
    RetainEdge,
}

impl RuntimeValue {
    /// The caller keeps `object` alive until this guard is dropped. Null is not
    /// a Python value; optional arguments must choose their None value explicitly.
    pub(crate) unsafe fn acquire(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        unsafe { GLOBAL_BRIDGE.acquire_runtime_value(object, RuntimeValueAccess::Observe) }
    }

    /// Keep only crossings that actually own a temporary foreign wrapper.
    /// Canonical managed values remain borrowed from the C caller's lifetime.
    /// The handle can be copied into a synchronous hook span independently of
    /// this optional guard; no representation or identity inference is exposed.
    pub(crate) fn into_temporary_owner(self) -> Option<Self> {
        self.owned.then_some(self)
    }

    /// Retain a reference edge without requiring a container's construction to
    /// be complete. Initialized direct-C slots and layout validity still commit
    /// through the same authority; only semantic reads require every slot.
    pub(crate) unsafe fn acquire_edge(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        unsafe { GLOBAL_BRIDGE.acquire_runtime_value(object, RuntimeValueAccess::RetainEdge) }
    }

    pub(crate) fn bits(&self) -> AbiHandle {
        self.bits
    }

    /// Adopt one already-owned runtime result. The caller must have validated
    /// the producing API's failure sentinel (in particular zero for handle-only
    /// allocation hooks); this constructor does not create or validate a value.
    pub(crate) unsafe fn from_owned(bits: AbiHandle) -> Self {
        Self { bits, owned: true }
    }

    pub(crate) fn into_owned_bits(mut self) -> AbiHandle {
        if !self.owned {
            unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(self.bits) };
        }
        self.owned = false;
        self.bits
    }
}

impl Drop for RuntimeValue {
    fn drop(&mut self) {
        if self.owned {
            crate::api::errors::with_preserved_error(|| unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(self.bits);
            });
        }
    }
}

/// Static publication failures captured at the locked admission boundary.
/// The facts describe the rejected transaction, not a later diagnostic lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticBindingError {
    InvalidInput {
        address: usize,
        bits: AbiHandle,
    },
    AddressIdentityConflict {
        forward: Option<AbiHandle>,
        direct: Option<AbiHandle>,
        foreign: Option<AbiHandle>,
        foreign_inflight: bool,
        numeric_carrier: Option<Option<AbiHandle>>,
    },
    CanonicalTargetManaged {
        address: usize,
    },
    CanonicalTargetBorrowed {
        address: usize,
    },
    ManagedPrevious {
        bits: AbiHandle,
        address: usize,
    },
}

impl ObjectBridge {
    #[inline(always)]
    pub fn pyobj_to_handle(&self, ptr: *mut PyObject) -> Option<BridgeIdentity> {
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            return Some(BridgeIdentity(bits));
        }
        let addr = ptr.addr();
        loop {
            let address = self.address_shard(addr).lock();
            if let Some(bits) = address
                .numeric_carriers
                .get(&addr)
                .and_then(|record| record.bits)
            {
                return Some(BridgeIdentity(bits));
            }
            let bits = address.from_py.get(&addr).copied()?;
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let visible = handle
                .raw_py
                .get(&bits)
                .is_some_and(|binding| binding.address == addr)
                || handle.to_py.get(&bits).is_some_and(|entry| {
                    entry.view.py_obj().addr() == addr
                        && (matches!(entry.publication, PublicationState::Ready)
                            || matches!(
                                &entry.publication,
                                PublicationState::Building { owner }
                                    if is_publication_owner(self, bits, owner)
                            ))
                });
            if visible {
                return Some(BridgeIdentity(bits));
            }
            let waiting = handle.to_py.get(&bits).is_some_and(|entry| {
                matches!(
                    entry.publication,
                    PublicationState::Building { .. } | PublicationState::Retiring
                )
            });
            drop(address);
            if !waiting {
                return None;
            }
            self.publication_ready[index].wait(&mut handle);
        }
    }
}

// Bidirectional identity, foreign-wrapper custody, and static registration.
impl ObjectBridge {
    pub(super) fn pyobj_matches_handle(&self, ptr: *mut PyObject, bits: AbiHandle) -> bool {
        if ptr.is_null() {
            return false;
        }
        // Exact direct bindings include noncanonical aliases. Container and
        // slice edges must retain the original C pointer without requiring it
        // to become the reverse-map canonical projection for this value.
        if self.molt_handle_for_pyobj(ptr).map(MoltValueHandle::bits) == Some(bits) {
            return true;
        }
        let address = self.address_shard(ptr.addr()).lock();
        address.foreign.get(&ptr.addr()).copied() == Some(bits)
    }

    pub fn molt_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<MoltValueHandle> {
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            return Some(MoltValueHandle(bits));
        }
        let addr = ptr.addr();
        let address = self.address_shard(addr).lock();
        if let Some(bits) = address
            .numeric_carriers
            .get(&addr)
            .and_then(|record| record.bits)
        {
            return Some(MoltValueHandle(bits));
        }
        if let Some(bits) = address.direct_molt_py.get(&addr).copied() {
            return Some(MoltValueHandle(bits));
        }
        drop(address);
        self.managed_handle_for_pyobj(ptr).map(MoltValueHandle)
    }

    /// Typed semantic ingress. Numeric adoption failure is not a membership
    /// miss and can never license foreign wrapping. Pure membership stays
    /// nonallocating for refcount/GC/layout checks.
    pub(super) fn semantic_handle_for_pyobj(
        &self,
        pointer: *mut PyObject,
    ) -> Result<Option<MoltValueHandle>, ()> {
        if let Some(bits) = pyobj_to_handle_static(pointer) {
            return Ok(Some(MoltValueHandle(bits)));
        }
        let address = self.address_shard(pointer.addr()).lock();
        let record = address.numeric_carriers.get(&pointer.addr()).copied();
        let Some(record) = record else {
            if let Some(bits) = address.direct_molt_py.get(&pointer.addr()).copied() {
                return Ok(Some(MoltValueHandle(bits)));
            }
            drop(address);
            return Ok(self.managed_handle_for_pyobj(pointer).map(MoltValueHandle));
        };
        drop(address);
        // Only a standalone numeric crossing needs adoption custody. Already
        // managed/direct/static values retain their existing lookup cost.
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        // Another admitted caller may have completed adoption while this one
        // waited for execution custody. Reuse that winner before allocating B.
        let current = self
            .address_shard(pointer.addr())
            .lock()
            .numeric_carriers
            .get(&pointer.addr())
            .copied();
        if current.is_none() {
            if let Some(bits) = self.managed_handle_for_pyobj(pointer) {
                return Ok(Some(MoltValueHandle(bits)));
            }
            unsafe { ensure_result_error(c"numeric source lost canonical ownership") };
            return Err(());
        }
        if current != Some(record) {
            unsafe { ensure_result_error(c"numeric source changed before adoption") };
            return Err(());
        }
        // Semantic admission cannot consume or replace either incoming raised
        // channel. Refuse before decoding/allocating a new runtime identity.
        if crate::api::errors::raised_error_pending() {
            return Err(());
        }
        if !crate::hooks::numeric_identity_available() {
            unsafe { ensure_result_error(c"registered runtime has no numeric identity owner") };
            return Err(());
        }
        let mut staged = None;
        let bits = if let Some(bits) = record.bits {
            bits
        } else {
            let Some(bits) =
                (unsafe { crate::api::numbers::decode_standalone_numeric(pointer, record.kind) })
            else {
                unsafe { ensure_result_error(c"invalid standalone numeric payload") };
                return Err(());
            };
            staged = Some(unsafe { RuntimeValue::from_owned(bits) });
            bits
        };
        let bits = if MoltObject::from_bits(bits).is_ptr() {
            bits
        } else {
            let result = unsafe { crate::hooks::hooks_or_stubs().numeric_identity_new(bits) };
            let crate::hooks::DecodedHandleResult::Ok(heap) = result.decode() else {
                unsafe { ensure_result_error(c"numeric identity allocation failed") };
                return Err(());
            };
            staged = Some(unsafe { RuntimeValue::from_owned(heap) });
            heap
        };
        // Raw integer decode has a separate pending-error channel. A producer
        // may allocate before a reentrant error is raised; staged custody must
        // retire that result instead of publishing it with an error pending.
        if crate::api::errors::raised_error_pending() {
            return Err(());
        }
        let expected = match record.kind {
            NumericCarrierKind::Long { .. } => MoltTypeTag::Int,
            NumericCarrierKind::Float => MoltTypeTag::Float,
            NumericCarrierKind::Complex => MoltTypeTag::Complex,
        };
        if !MoltObject::from_bits(bits).is_ptr() || Self::classify_handle(bits) != expected {
            unsafe {
                ensure_result_error(c"numeric identity producer returned the wrong heap kind")
            };
            return Err(());
        }
        let Some(entry) = super::publication::PendingNumericEntry::new(pointer, record, bits)
        else {
            return Err(());
        };
        if let Err((entry, reason)) = self.insert_managed_entry(bits, entry) {
            drop(entry); // only uninitialized entry storage; never the source allocation
            if matches!(
                reason,
                super::publication::ManagedEntryRejection::InvalidNumericTransfer
            ) && let Some(winner) = self.managed_handle_for_pyobj(pointer)
            {
                // A competing/reentrant admission won this same source while
                // staging allocated B. Reuse its canonical committed identity;
                // the local staged owner retires normally. This is not a retry,
                // foreign fallback, or acceptance of a failed owner producer.
                return Ok(Some(MoltValueHandle(winner)));
            }
            unsafe { reason.set_error() };
            return Err(());
        }
        // Commit transferred the existing record's hold, or the newly staged
        // hold. Disarming a local guard is the only remaining operation.
        if let Some(owner) = staged {
            owner.into_owned_bits();
        }
        Ok(Some(MoltValueHandle(bits)))
    }

    /// Resolve a C object for semantic runtime observation. Unlike the raw
    /// identity lookup, this commits every mutable physical projection first,
    /// so generic protocols cannot observe stale list/exception state merely
    /// because they bypassed a type-specific C API entry point.
    pub fn observed_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<MoltValueHandle> {
        let value = self.semantic_handle_for_pyobj(ptr).ok()??;
        self.prepare_runtime_value(value, RuntimeValueAccess::Observe)
    }

    pub(super) fn prepare_runtime_value(
        &self,
        value: MoltValueHandle,
        access: RuntimeValueAccess,
    ) -> Option<MoltValueHandle> {
        if !observe_publication(self, value.bits()) {
            unsafe { ensure_result_error(c"managed projection is retiring") };
            return None;
        }
        match Self::classify_handle(value.bits()) {
            MoltTypeTag::Str if !self.commit_unicode_view(value.bits()) => return None,
            MoltTypeTag::Exception if !self.commit_exception_view(value.bits()) => return None,
            MoltTypeTag::List
                if !self.commit_list_view_inner(
                    value.bits(),
                    matches!(access, RuntimeValueAccess::Observe),
                ) =>
            {
                return None;
            }
            _ => {}
        }
        Some(value)
    }

    /// Resolve only the canonical ABI view owned by a live Molt heap object.
    /// Static singletons, scalar layout carriers, and foreign objects are not
    /// managed views and retain their native deallocation authority.
    pub fn managed_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        if ptr.is_null() {
            return None;
        }
        let addr = ptr.addr();
        loop {
            let address = self.address_shard(addr).lock();
            let bits = address.from_py.get(&addr).copied()?;
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let entry = handle.to_py.get(&bits)?;
            if entry.view.py_obj().addr() != addr {
                return None;
            }
            match &entry.publication {
                PublicationState::Ready => return Some(bits),
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                    return Some(bits);
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                }
            }
        }
    }

    pub unsafe fn molt_value_for_pyobj(&self, ptr: *mut PyObject) -> Option<u64> {
        if ptr.is_null() {
            return None;
        }
        unsafe { self.acquire_runtime_value(ptr, RuntimeValueAccess::Observe) }
            .map(RuntimeValue::into_owned_bits)
    }

    pub(super) unsafe fn acquire_runtime_value(
        &self,
        ptr: *mut PyObject,
        access: RuntimeValueAccess,
    ) -> Option<RuntimeValue> {
        if let Some(value) = self.semantic_handle_for_pyobj(ptr).ok()? {
            // A failed semantic commit is not evidence of foreign identity.
            return self
                .prepare_runtime_value(value, access)
                .map(|value| RuntimeValue {
                    bits: value.bits(),
                    owned: false,
                });
        }
        admit_foreign_pyobject(ptr)?;
        #[cfg(test)]
        static_binding_transaction_tests::pause_before_foreign_reservation();
        unsafe { self.foreign_wrapper_for(ptr, access) }
    }

    pub(super) unsafe fn foreign_wrapper_for(
        &self,
        ptr: *mut PyObject,
        access: RuntimeValueAccess,
    ) -> Option<RuntimeValue> {
        let key = ptr.expose_provenance();
        let hooks = crate::hooks::hooks_or_stubs();
        let address_index = self.address_shard_index(key);
        let mut address = self.address_shards[address_index].lock();
        loop {
            // Canonical publication may have won after the optimistic lookup
            // but before this reservation. Once reserved, static raw
            // publication rejects foreign_inflight under this same lock.
            if address.from_py.contains_key(&key)
                || address.direct_molt_py.contains_key(&key)
                || address.numeric_carriers.contains_key(&key)
            {
                drop(address);
                let value = self
                    .semantic_handle_for_pyobj(ptr)
                    .ok()?
                    .and_then(|value| self.prepare_runtime_value(value, access));
                if let Some(value) = value {
                    return Some(RuntimeValue {
                        bits: value.bits(),
                        owned: false,
                    });
                }
                unsafe {
                    ensure_result_error(c"registered ABI object could not yield its runtime value")
                };
                return None;
            }
            if let Some(wrapper) = address.foreign.get(&key).copied() {
                drop(address);
                unsafe { (hooks.inc_ref)(wrapper) };
                return Some(RuntimeValue {
                    bits: wrapper,
                    owned: true,
                });
            }
            if address.foreign_inflight.insert(key) {
                break;
            }
            self.foreign_ready[address_index].wait(&mut address);
        }
        drop(address);

        let wrapper = unsafe { (hooks.foreign_new)(key) };
        if wrapper == 0 {
            let mut address = self.address_shards[address_index].lock();
            address.foreign_inflight.remove(&key);
            self.foreign_ready[address_index].notify_all();
            drop(address);
            unsafe { ensure_result_error(c"foreign runtime value acquisition failed") };
            return None;
        }

        // Acquire the C custody edge before taking bridge publication locks.
        // Py_INCREF probes managed membership and therefore re-enters the
        // address shard; doing it under `lock_address_then_handle` deadlocks on
        // the first genuine foreign crossing.
        unsafe { crate::api::refcount::Py_INCREF(ptr) };
        let (mut address, mut handle) = self.lock_address_then_handle(key, wrapper);
        if handle.raw_py.contains_key(&wrapper) || handle.to_py.contains_key(&wrapper) {
            address.foreign_inflight.remove(&key);
            self.foreign_ready[address_index].notify_all();
            drop(handle);
            drop(address);
            // A faulty producer must not replace another binding or abandon
            // either the returned owned wrapper or the C custody edge.
            crate::api::errors::with_preserved_error(|| unsafe { (hooks.dec_ref)(wrapper) });
            unsafe { crate::api::errors::release_preserving_error(&[ptr]) };
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                    c"foreign wrapper producer returned an existing bridge identity".as_ptr(),
                );
            }
            return None;
        }
        address.foreign.insert(key, wrapper);
        address.foreign_inflight.remove(&key);
        handle.raw_py.insert(wrapper, RawBinding::borrowed(key));
        self.foreign_ready[address_index].notify_all();
        Some(RuntimeValue {
            bits: wrapper,
            owned: true,
        })
    }

    pub unsafe fn release_foreign(&self, c_ptr: usize) {
        let mut address = self.address_shard(c_ptr).lock();
        let Some(wrapper) = address.foreign.get(&c_ptr).copied() else {
            return;
        };
        let mut handle = self.handle_shard(wrapper).lock();
        address.foreign.remove(&c_ptr);
        if handle.raw_py.get(&wrapper) == Some(&RawBinding::borrowed(c_ptr)) {
            handle.raw_py.remove(&wrapper);
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_foreign_for_test(&self, ptr: *mut PyObject, handle_bits: AbiHandle) {
        let exposed_addr = ptr.expose_provenance();
        let (mut address, mut handle) = self.lock_address_then_handle(ptr.addr(), handle_bits);
        address.foreign.insert(ptr.addr(), handle_bits);
        handle
            .raw_py
            .insert(handle_bits, RawBinding::borrowed(exposed_addr));
    }

    /// Rebind a canonical static C object (notably `PyExc_*`) from its
    /// bootstrap binding to a real runtime handle. Ingress always
    /// uses `direct_molt_py`; when `canonical_view` is true, handle-to-PyObject
    /// projection also resolves to this immortal static pointer. Publication
    /// atomically validates and updates both directions under the address and
    /// affected handle locks; a canonical reverse binding cannot replace an
    /// existing managed view or another raw pointer.
    pub unsafe fn bind_static_pyobj_to_runtime_handle(
        &self,
        ptr: *mut PyObject,
        bits: AbiHandle,
        canonical_view: bool,
    ) -> Result<(), StaticBindingError> {
        if ptr.is_null() || bits == 0 {
            return Err(StaticBindingError::InvalidInput {
                address: ptr.addr(),
                bits,
            });
        }
        let addr = ptr.addr();
        let mut address = self.address_shard(addr).lock();
        let old_bits = address.from_py.get(&addr).copied();
        // A static identity may replace only an exact direct binding, never
        // a managed view, numeric carrier, or foreign-wrapper identity.
        if address.direct_molt_py.get(&addr).copied() != old_bits
            || address.foreign.contains_key(&addr)
            || address.foreign_inflight.contains(&addr)
            || address.numeric_carriers.contains_key(&addr)
        {
            return Err(StaticBindingError::AddressIdentityConflict {
                forward: old_bits,
                direct: address.direct_molt_py.get(&addr).copied(),
                foreign: address.foreign.get(&addr).copied(),
                foreign_inflight: address.foreign_inflight.contains(&addr),
                numeric_carrier: address
                    .numeric_carriers
                    .get(&addr)
                    .map(|record| record.bits),
            });
        }
        let target_index = self.handle_shard_index(bits);
        let old_index = old_bits.map_or(target_index, |old| self.handle_shard_index(old));
        // Address rank first, then each affected handle rank exactly once in
        // index order. Validation and every map edit share this lock set.
        let first_index = old_index.min(target_index);
        let second_index = old_index.max(target_index);
        let mut first = self.handle_shards[first_index].lock();
        let mut second =
            (first_index != second_index).then(|| self.handle_shards[second_index].lock());
        {
            let target = if target_index == first_index {
                &*first
            } else {
                second.as_deref().unwrap()
            };
            let raw = target.raw_py.get(&bits).copied();
            if canonical_view {
                if let Some(address) = target.managed_address(bits) {
                    return Err(StaticBindingError::CanonicalTargetManaged { address });
                }
                if let Some(binding) = raw
                    && binding != RawBinding::borrowed(addr)
                {
                    return Err(StaticBindingError::CanonicalTargetBorrowed {
                        address: binding.address,
                    });
                }
            }
        }
        if let Some(old_bits) = old_bits {
            let old = if old_index == first_index {
                &mut *first
            } else {
                second.as_deref_mut().unwrap()
            };
            if old.managed_address(old_bits) == Some(addr) {
                return Err(StaticBindingError::ManagedPrevious {
                    bits: old_bits,
                    address: addr,
                });
            }
            if old_bits != bits && old.raw_py.get(&old_bits) == Some(&RawBinding::borrowed(addr)) {
                old.raw_py.remove(&old_bits);
            }
        }
        address.from_py.insert(addr, bits);
        address.direct_molt_py.insert(addr, bits);
        #[cfg(test)]
        static_binding_transaction_tests::pause_after_forward_publication();
        if canonical_view {
            let target = if target_index == first_index {
                &mut *first
            } else {
                second.as_deref_mut().unwrap()
            };
            target.raw_py.insert(bits, RawBinding::borrowed(addr));
        }
        Ok(())
    }

    /// Retire one exact static/direct C-object binding without changing any
    /// reference ownership.
    ///
    /// Static bindings occupy both address-keyed ingress maps.  Requiring the
    /// supplied pair to match in both maps and excluding an exact managed
    /// address protects managed views. Conditional reverse removal preserves a
    /// newer canonical pointer or an independent alias for the same runtime handle. The
    /// runtime owns any strong class anchor associated with this identity and
    /// must retire that anchor separately after unbinding.
    ///
    /// Returns `false` without mutation for null/zero inputs or when the exact
    /// forward pair is no longer current.  A missing reverse mapping is valid:
    /// bindings created with `canonical_view == false` intentionally have no
    /// handle-to-pointer entry.
    pub unsafe fn unbind_static_pyobj_from_runtime_handle(
        &self,
        ptr: *mut PyObject,
        bits: AbiHandle,
    ) -> bool {
        if ptr.is_null() || bits == 0 {
            return false;
        }
        let addr = ptr.addr();
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        if address.from_py.get(&addr).copied() != Some(bits)
            || address.direct_molt_py.get(&addr).copied() != Some(bits)
            || handle.managed_address(bits) == Some(addr)
        {
            return false;
        }
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        if handle.raw_py.get(&bits) == Some(&RawBinding::borrowed(addr)) {
            handle.raw_py.remove(&bits);
        }
        true
    }

    pub(crate) fn register_numeric_carrier(
        &self,
        ptr: *mut PyObject,
        bits: Option<AbiHandle>,
        kind: NumericCarrierKind,
    ) -> bool {
        use super::publication::ManagedEntryRejection;
        let mut address = self.address_shard(ptr.addr()).lock();
        let error = if ptr.is_null()
            || address.numeric_carriers.contains_key(&ptr.addr())
            || address.from_py.contains_key(&ptr.addr())
            || address.foreign.contains_key(&ptr.addr())
            || address.foreign_inflight.contains(&ptr.addr())
        {
            Some(ManagedEntryRejection::InvalidNumericTransfer)
        } else if address.numeric_carriers.try_reserve(1).is_err() {
            Some(ManagedEntryRejection::NoMemory)
        } else {
            address
                .numeric_carriers
                .insert(ptr.addr(), NumericCarrierRecord { bits, kind });
            None
        };
        drop(address);
        if let Some(error) = error {
            unsafe { error.set_error() };
            false
        } else {
            true
        }
    }
}

/// Stateless `*mut PyObject` → Molt handle translation for static singletons.
///
/// Recognises `Py_None` / `Py_True` / `Py_False` directly.  Returns `None`
/// for non-singleton pointers; callers use the explicit bridge registries.
///
/// Pointer-equality only — no dereference — so this function is safe to
/// call with any `*mut PyObject` value (including dangling).
pub(super) fn pyobj_to_handle_static(ptr: *mut PyObject) -> Option<AbiHandle> {
    if ptr.is_null() {
        return None;
    }
    if std::ptr::eq(ptr, &raw const Py_None as *const _) {
        return Some(MoltObject::none().bits());
    }
    if std::ptr::eq(ptr, &raw const Py_True as *const _) {
        return Some(MoltObject::from_bool(true).bits());
    }
    if std::ptr::eq(ptr, &raw const Py_False as *const _) {
        return Some(MoltObject::from_bool(false).bits());
    }
    if let Some(bits) = crate::api::numbers::cached_small_int_bits_from_ptr(ptr) {
        return Some(bits);
    }
    // None still has a legacy Rust-side storage name. Bool has no second lane:
    // `Py_True`/`Py_False` are Rust aliases of the canonical `_Py_*Struct`
    // storage already checked above.
    if std::ptr::eq(
        ptr,
        &raw const crate::api::object::_Py_NoneStruct as *const _,
    ) {
        return Some(MoltObject::none().bits());
    }
    None
}

#[cfg(test)]
mod static_binding_transaction_tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    type Pause = (mpsc::SyncSender<()>, mpsc::Receiver<()>);
    thread_local! {
        static PAUSE: RefCell<Option<Pause>> = const { RefCell::new(None) };
        static FOREIGN_PAUSE: RefCell<Option<Pause>> = const { RefCell::new(None) };
    }

    pub(super) fn pause_after_forward_publication() {
        PAUSE.with(|slot| {
            if let Some((arrived, resume)) = slot.borrow_mut().take() {
                arrived.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
    }

    pub(super) fn pause_before_foreign_reservation() {
        FOREIGN_PAUSE.with(|slot| {
            if let Some((arrived, resume)) = slot.borrow_mut().take() {
                arrived.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
    }

    #[test]
    fn foreign_reservation_revalidates_canonical_publication_after_lookup_miss() {
        let bridge = Arc::new(ObjectBridge::new());
        let address = 0x89000usize;
        let bits = MoltObject::from_int(23_004).bits();
        let (arrived_tx, arrived_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let crossing = Arc::clone(&bridge);
        let worker = thread::spawn(move || {
            FOREIGN_PAUSE.with(|slot| *slot.borrow_mut() = Some((arrived_tx, resume_rx)));
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(address);
            unsafe { crossing.acquire_runtime_value(ptr, RuntimeValueAccess::Observe) }
                .map(|value| (value.bits(), value.owned))
        });
        arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(address);
        assert!(unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, true) }.is_ok());
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Some((bits, false)));
        {
            let shard = bridge.address_shard(address).lock();
            assert_eq!(shard.direct_molt_py.get(&address), Some(&bits));
            assert!(!shard.foreign.contains_key(&address));
            assert!(!shard.foreign_inflight.contains(&address));
        }
        assert_eq!(
            bridge.handle_shard(bits).lock().raw_py.get(&bits).copied(),
            Some(RawBinding::borrowed(address))
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
    }

    fn rebind_with_competitor(old_bits: AbiHandle, bits: AbiHandle, managed_publication: bool) {
        let bridge = Arc::new(ObjectBridge::new());
        let addr = 0x88000usize;
        let candidate = managed_publication.then(|| {
            Box::new(BridgeEntry {
                view: ManagedView::Object(Box::new(BridgeHeader {
                    py_obj: UnsafeCell::new(PyObject {
                        ob_refcnt: 1,
                        ob_type: std::ptr::null_mut(),
                    }),
                })),
                bits,
                unicode: None,
                publication: PublicationState::Ready,
                lifecycle: BridgeLifecycle::ViewHoldOnly,
            })
        });
        let other_addr = candidate.as_ref().map(|entry| entry.view.py_obj().addr());
        let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, old_bits, true)
                .is_ok()
        });
        let (arrived_tx, arrived_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let publisher = Arc::clone(&bridge);
        let writer = thread::spawn(move || {
            PAUSE.with(|slot| *slot.borrow_mut() = Some((arrived_tx, resume_rx)));
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            unsafe {
                publisher
                    .bind_static_pyobj_to_runtime_handle(ptr, bits, true)
                    .is_ok()
            }
        });
        arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // At the old split-publication boundary, all three identities must
        // still be locked. This asserts exclusion without scheduler timing.
        assert!(bridge.address_shard(addr).try_lock().is_none());
        assert!(bridge.handle_shard(old_bits).try_lock().is_none());
        assert!(bridge.handle_shard(bits).try_lock().is_none());
        let competing = Arc::clone(&bridge);
        let start = Arc::new(Barrier::new(2));
        let competing_start = Arc::clone(&start);
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let contender = thread::spawn(move || {
            competing_start.wait();
            let result = if let Some(candidate) = candidate {
                // Exercise the sole managed-view insertion transaction. The
                // competing canonical binding must reject this physical entry
                // before runtime hooks or any publication can run.
                match competing.insert_managed_entry(bits, candidate) {
                    Err((rejected, ManagedEntryRejection::Occupied)) => {
                        assert_eq!(rejected.bits, bits);
                        assert_eq!(Some(rejected.view.py_obj().addr()), other_addr);
                        // The transaction returned custody after its locks
                        // dropped; this unpublished header owns no runtime hold.
                        drop(rejected);
                        false
                    }
                    Err((_, rejection)) => panic!("unexpected rejection: {rejection:?}"),
                    Ok(()) => panic!("managed view replaced a canonical static binding"),
                }
            } else {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
                unsafe { competing.unbind_static_pyobj_from_runtime_handle(ptr, bits) }
            };
            done_tx.send(result).unwrap();
        });
        start.wait();
        resume_tx.send(()).unwrap();
        assert!(writer.join().unwrap());
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            !managed_publication
        );
        contender.join().unwrap();
        assert!(
            !bridge
                .handle_shard(old_bits)
                .lock()
                .raw_py
                .contains_key(&old_bits)
        );
        if managed_publication {
            assert_eq!(
                bridge.handle_shard(bits).lock().raw_py.get(&bits).copied(),
                Some(RawBinding::borrowed(addr))
            );
            assert_eq!(
                bridge.address_shard(addr).lock().from_py.get(&addr),
                Some(&bits)
            );
            assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
            assert_eq!(
                bridge.address_shard(addr).lock().direct_molt_py.get(&addr),
                Some(&bits)
            );
            let other_addr = other_addr.expect("managed competitor has a physical header");
            {
                let rejected_address = bridge.address_shard(other_addr).lock();
                assert!(!rejected_address.from_py.contains_key(&other_addr));
                assert!(!rejected_address.direct_molt_py.contains_key(&other_addr));
            }
            assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        } else {
            let address = bridge.address_shard(addr).lock();
            assert!(!address.from_py.contains_key(&addr));
            assert!(!address.direct_molt_py.contains_key(&addr));
            assert!(!bridge.handle_shard(bits).lock().raw_py.contains_key(&bits));
        }
    }

    fn rebind_cases() -> Vec<(AbiHandle, AbiHandle)> {
        let count = ObjectBridge::new().shard_count() as u64;
        // Distinct identities sharing one shard exercise deduplication.
        let mut cases = vec![(0x1000, 0x1000 + count * 0x10)];
        if count > 1 {
            // Both orders exercise deterministic ordering of distinct shards.
            cases.extend([(0x1000, 0x1010), (0x1010, 0x1000)]);
        }
        cases
    }

    #[test]
    fn static_rebinding_serializes_managed_publication() {
        for (old_bits, bits) in rebind_cases() {
            rebind_with_competitor(old_bits, bits, true);
        }
    }

    #[test]
    fn static_rebinding_serializes_unbinding_without_orphan_reverse_entries() {
        for (old_bits, bits) in rebind_cases() {
            rebind_with_competitor(old_bits, bits, false);
        }
    }
}

#[cfg(test)]
mod bridge_concurrency_tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn stripe_count_and_hashes_follow_the_design() {
        let bridge = ObjectBridge::new();
        #[cfg(target_arch = "wasm32")]
        assert_eq!(bridge.shard_count(), 1);
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(
            bridge.shard_count(),
            std::thread::available_parallelism()
                .map_or(1, usize::from)
                .saturating_mul(2)
                .next_power_of_two()
        );
        let mask = bridge.shard_count() - 1;
        assert_eq!(
            bridge.address_shard_index(0x1234_5670),
            (0x0123_4567_usize) & mask
        );
        assert_eq!(
            bridge.handle_shard_index(0x7ff8_1234_5678_9ab0),
            (0x07ff_8123_4567_89ab_usize) & mask
        );
    }

    #[test]
    fn crossed_stripes_obey_address_then_handle_rank_without_deadlock() {
        let bridge = Arc::new(ObjectBridge::new());
        if bridge.shard_count() == 1 {
            return;
        }
        let barrier = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = mpsc::channel();
        for (addr, bits) in [(0x10usize, 0x20u64), (0x20usize, 0x10u64)] {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let done_tx = done_tx.clone();
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..100_000 {
                    let (_address, _handle) = bridge.lock_address_then_handle(addr, bits);
                }
                done_tx.send(()).expect("rank stress receiver dropped");
            });
        }
        for _ in 0..2 {
            done_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("crossed stripe acquisition deadlocked");
        }
    }

    #[test]
    fn disjoint_crossing_and_release_preserve_bidirectional_identity() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let thread_count = thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(2, 16);
        let barrier = Arc::new(Barrier::new(thread_count));
        let (done_tx, done_rx) = mpsc::channel();
        for thread_index in 0..thread_count {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let done_tx = done_tx.clone();
            thread::spawn(move || {
                barrier.wait();
                for iteration in 0..2_000usize {
                    let ordinal = thread_index * 2_000 + iteration + 1;
                    let address = 0x1_0000usize + ordinal * 16;
                    let heap_bits = MoltObject::from_ptr(address as *mut u8).bits();
                    let value = 1_000 + (thread_index * 2_000 + iteration) as i64;
                    let numeric_bits = MoltObject::from_int(value).bits();
                    // Exercise both heap-handle and non-small numeric managed
                    // views through the one publication/release authority.
                    for bits in [heap_bits, numeric_bits] {
                        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
                        assert_eq!(
                            bridge.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle),
                            Some(bits)
                        );
                        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
                    }
                }
                done_tx.send(()).expect("crossing stress receiver dropped");
            });
        }
        for _ in 0..thread_count {
            done_rx
                .recv_timeout(Duration::from_secs(20))
                .expect("concurrent bridge crossing deadlocked");
        }
    }

    #[test]
    fn small_int_crossings_are_stateless_immortal_and_deterministic() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        for value in -5..=256 {
            let bits = MoltObject::from_int(value).bits();
            let first = unsafe { bridge.owned_handle_to_pyobj(bits) };
            let second = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
            assert_eq!(first, second);
            assert!(unsafe { crate::abi_types::is_immortal_refcnt((*first).ob_refcnt) });
            assert_eq!(
                bridge.pyobj_to_handle(first).map(BridgeIdentity::as_handle),
                Some(bits)
            );
            assert_eq!(bridge.release_pyobj(first), PyObjRelease::StaticImmortal);
        }
        assert!(
            bridge
                .address_shards
                .iter()
                .all(|shard| shard.lock().from_py.is_empty())
        );
        assert!(
            bridge
                .handle_shards
                .iter()
                .all(|shard| shard.lock().to_py.is_empty())
        );
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;
    /// Regression for the numpy `_multiarray_umath` "'str' object is not
    /// callable" frontier: `PyObject_Call` routes bridge-managed Molt callables
    /// to the runtime `object_call` hook via `molt_handle_for_pyobj`, which must
    /// return genuine Molt handles for minted proxies and MUST NOT hand a
    /// raw-registry synthetic handle (not valid `MoltObject` bits) to the
    /// runtime.
    #[test]
    fn molt_handle_for_pyobj_excludes_raw_registered_pointers() {
        let _thread = crate::api::object::AbiTestThreadStateTransaction::new();
        init_tag_table();
        let bridge = &*GLOBAL_BRIDGE;
        // Minted proxy for a genuine Molt handle resolves through both paths.
        let int_bits = MoltObject::from_int(0x5EED).bits();
        let proxy = unsafe { bridge.owned_handle_to_pyobj(int_bits) };
        assert_eq!(
            bridge.pyobj_to_handle(proxy).map(BridgeIdentity::as_handle),
            Some(int_bits)
        );
        assert_eq!(
            bridge
                .molt_handle_for_pyobj(proxy)
                .map(MoltValueHandle::bits),
            Some(int_bits)
        );
        // Without a runtime foreign-object hook, an arbitrary C object remains
        // unregistered; no synthetic non-Molt identity is fabricated.
        let mut stray = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let stray_ptr = &raw mut stray;
        assert_eq!(unsafe { bridge.molt_value_for_pyobj(stray_ptr) }, None);
        assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
        assert!(bridge.pyobj_to_handle(stray_ptr).is_none());
        assert_eq!(bridge.molt_handle_for_pyobj(stray_ptr), None);
        unsafe {
            crate::api::errors::PyErr_Clear();
            crate::api::refcount::Py_DECREF(proxy);
        }
    }

    /// `molt_value_for_pyobj` resolves static singletons and genuine Molt
    /// proxies to their canonical Molt handles WITHOUT foreign-wrapping them —
    /// only genuine C-extension objects get a `TYPE_ID_FOREIGN` wrapper.
    #[test]
    fn molt_value_for_pyobj_resolves_singletons_and_proxies() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        // Static singleton `None` → canonical NaN-boxed None, no wrapper.
        let none_ptr = &raw mut Py_None;
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(none_ptr) },
            Some(MoltObject::none().bits())
        );
        // A genuine Molt object that crossed to C (a bridge proxy) resolves back
        // to its own Molt handle, not a foreign wrapper.
        let int_bits = MoltObject::from_int(0x1234).bits();
        let proxy = unsafe { bridge.owned_handle_to_pyobj(int_bits) };
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(proxy) },
            Some(int_bits)
        );
        // A process-static shell can acquire successive runtime bindings;
        // canonical ingress must never create a foreign identity for either.
        let mut shell = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let shell_ptr = &raw mut shell;
        for value in [20_004, 20_005] {
            let bits = MoltObject::from_int(value).bits();
            assert!(
                unsafe { bridge.bind_static_pyobj_to_runtime_handle(shell_ptr, bits, true) }
                    .is_ok()
            );
            assert_eq!(
                unsafe { bridge.molt_value_for_pyobj(shell_ptr) },
                Some(bits)
            );
            assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(shell_ptr, bits) });
        }
        assert!(
            bridge
                .address_shards
                .iter()
                .all(|shard| shard.lock().foreign.is_empty())
        );
    }

    /// A foreign wrapper's identity round-trips: handed back to C it resolves to
    /// the ORIGINAL C pointer (via `raw_py`), and `release_foreign` drops both
    /// the `foreign` and `raw_py` identity entries.
    #[test]
    fn foreign_wrapper_round_trips_and_releases() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let mut fake = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let c_ptr = &raw mut fake;
        // Stand in for a minted `TYPE_ID_FOREIGN` wrapper handle (the runtime
        // hook is not linked in a pure-ABI test); install the identity entries
        // exactly as `foreign_wrapper_for` would.
        let w_bits = 0xBEEF_0000_0000_0010u64;
        // Expose once (the address is reconstructed into a pointer by
        // `handle_to_pyobj` below), then use it for the identity entries exactly
        // as `foreign_wrapper_for` does.
        let addr = c_ptr.expose_provenance();
        bridge.insert_foreign_for_test(c_ptr, w_bits);
        // The wrapper handed back to C resolves to the original C pointer.
        let back = unsafe { bridge.owned_handle_to_pyobj(w_bits) };
        assert_eq!(
            back, c_ptr,
            "foreign wrapper must round-trip to its C object"
        );
        // Release drops the identity mapping so a fresh wrapper can be minted.
        unsafe { bridge.release_foreign(addr) };
        assert!(
            !bridge
                .address_shard(addr)
                .lock()
                .foreign
                .contains_key(&addr)
        );
        assert!(
            !bridge
                .handle_shard(w_bits)
                .lock()
                .raw_py
                .contains_key(&w_bits)
        );
    }

    #[test]
    fn static_binding_unbind_retires_exact_canonical_pair() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let bits = 0xA110_0000_0000_0010;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, bits, true)
                .is_ok()
        });
        assert_eq!(
            bridge.address_shard(addr).lock().from_py.get(&addr),
            Some(&bits)
        );
        assert_eq!(
            bridge.address_shard(addr).lock().direct_molt_py.get(&addr),
            Some(&bits)
        );
        assert_eq!(
            bridge
                .handle_shard(bits)
                .lock()
                .raw_py
                .get(&bits)
                .map(|binding| &binding.address),
            Some(&addr)
        );

        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        let address = bridge.address_shard(addr).lock();
        assert!(!address.from_py.contains_key(&addr));
        assert!(!address.direct_molt_py.contains_key(&addr));
        drop(address);
        assert!(!bridge.handle_shard(bits).lock().raw_py.contains_key(&bits));
    }

    #[test]
    fn static_binding_unbind_accepts_noncanonical_alias_without_reverse() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let bits = 0xA110_0000_0000_0020;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, bits, false)
                .is_ok()
        });
        assert!(!bridge.handle_shard(bits).lock().raw_py.contains_key(&bits));
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        let address = bridge.address_shard(addr).lock();
        assert!(!address.from_py.contains_key(&addr));
        assert!(!address.direct_molt_py.contains_key(&addr));
    }

    #[test]
    fn static_alias_release_without_reverse_detaches_before_republication() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let old_bits = 0xA110_0000_0000_0090;
        let new_bits = 0xA110_0000_0000_00A0;
        assert!(
            unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, old_bits, false) }.is_ok()
        );
        assert_eq!(
            bridge.release_pyobj(ptr),
            PyObjRelease::DirectViewUnregistered
        );
        {
            let address = bridge.address_shard(addr).lock();
            assert!(!address.from_py.contains_key(&addr));
            assert!(!address.direct_molt_py.contains_key(&addr));
        }
        assert!(unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, new_bits, true) }.is_ok());
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, old_bits) });
        assert_eq!(
            bridge.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle),
            Some(new_bits)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, new_bits) });
    }

    #[test]
    fn static_alias_release_preserves_managed_canonical_projection() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(20_002).bits();
        let managed = unsafe { bridge.owned_handle_to_pyobj(bits) };
        // Direct semantic ingress is not a transfer of managed ownership.
        bridge
            .address_shard(managed.addr())
            .lock()
            .direct_molt_py
            .insert(managed.addr(), bits);
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(managed, bits) });
        let replacement = MoltObject::from_int(20_003).bits();
        for canonical in [false, true] {
            assert_eq!(
                unsafe {
                    bridge.bind_static_pyobj_to_runtime_handle(managed, replacement, canonical)
                },
                Err(StaticBindingError::ManagedPrevious {
                    bits,
                    address: managed.addr()
                })
            );
        }
        let mut alias = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut alias;
        assert_eq!(
            unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, true) },
            Err(StaticBindingError::CanonicalTargetManaged {
                address: managed.addr()
            })
        );
        assert!(unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, false) }.is_ok());
        assert_eq!(
            bridge.release_pyobj(ptr),
            PyObjRelease::DirectViewUnregistered
        );
        assert_eq!(bridge.managed_handle_for_pyobj(managed), Some(bits));
        assert_eq!(unsafe { bridge.handle_to_borrowed_pyobj(bits) }, managed);
        assert_eq!(
            bridge
                .address_shard(managed.addr())
                .lock()
                .direct_molt_py
                .get(&managed.addr())
                .copied(),
            Some(bits)
        );
        assert!(bridge.molt_handle_for_pyobj(ptr).is_none());
        assert_eq!(
            bridge.release_pyobj(managed),
            PyObjRelease::ManagedViewRetired
        );
        assert!(
            !bridge
                .address_shard(managed.addr())
                .lock()
                .direct_molt_py
                .contains_key(&managed.addr())
        );
    }

    #[test]
    fn static_binding_unbind_preserves_newer_forward_identity() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let old_bits = 0xA110_0000_0000_0030;
        let new_bits = 0xA110_0000_0000_0040;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, old_bits, true)
                .is_ok()
        });
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, new_bits, true)
                .is_ok()
        });
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, old_bits) });
        let address = bridge.address_shard(addr).lock();
        assert_eq!(address.from_py.get(&addr), Some(&new_bits));
        assert_eq!(address.direct_molt_py.get(&addr), Some(&new_bits));
        drop(address);
        assert_eq!(
            bridge
                .handle_shard(new_bits)
                .lock()
                .raw_py
                .get(&new_bits)
                .map(|binding| &binding.address),
            Some(&addr)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, new_bits) });
    }

    #[test]
    fn static_binding_unbind_rejects_inconsistent_forward_pair_without_mutation() {
        let bridge = ObjectBridge::new();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let ptr = &raw mut object;
        let addr = ptr.addr();
        let bits = 0xA110_0000_0000_0048;
        let conflicting_bits = 0xA110_0000_0000_0049;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(ptr, bits, true)
                .is_ok()
        });
        bridge
            .address_shard(addr)
            .lock()
            .direct_molt_py
            .insert(addr, conflicting_bits);
        assert!(!unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
        let address = bridge.address_shard(addr).lock();
        assert_eq!(address.from_py.get(&addr), Some(&bits));
        assert_eq!(address.direct_molt_py.get(&addr), Some(&conflicting_bits));
        drop(address);
        assert_eq!(
            unsafe { bridge.bind_static_pyobj_to_runtime_handle(ptr, bits, true) },
            Err(StaticBindingError::AddressIdentityConflict {
                forward: Some(bits),
                direct: Some(conflicting_bits),
                foreign: None,
                foreign_inflight: false,
                numeric_carrier: None,
            })
        );
        let mut address = bridge.address_shard(addr).lock();
        assert_eq!(address.from_py.get(&addr), Some(&bits));
        assert_eq!(address.direct_molt_py.get(&addr), Some(&conflicting_bits));
        address.direct_molt_py.insert(addr, bits);
        drop(address);
        assert_eq!(
            bridge
                .handle_shard(bits)
                .lock()
                .raw_py
                .get(&bits)
                .map(|binding| &binding.address),
            Some(&addr)
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(ptr, bits) });
    }

    #[test]
    fn static_binding_unbind_preserves_newer_reverse_identity() {
        let bridge = ObjectBridge::new();
        let mut canonical = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let mut alias = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let canonical_ptr = &raw mut canonical;
        let alias_ptr = &raw mut alias;
        let canonical_addr = canonical_ptr.addr();
        let alias_addr = alias_ptr.addr();
        let bits = 0xA110_0000_0000_0050;

        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(canonical_ptr, bits, true)
                .is_ok()
        });
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(alias_ptr, bits, false)
                .is_ok()
        });
        assert!(bridge.pyobj_matches_handle(canonical_ptr, bits));
        assert!(bridge.pyobj_matches_handle(alias_ptr, bits));
        let rebound_bits = bits + 1;
        assert!(!bridge.pyobj_matches_handle(alias_ptr, rebound_bits));
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(alias_ptr, rebound_bits, false)
                .is_ok()
        });
        assert!(!bridge.pyobj_matches_handle(alias_ptr, bits));
        assert!(bridge.pyobj_matches_handle(alias_ptr, rebound_bits));
        assert!(bridge.pyobj_matches_handle(canonical_ptr, bits));
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(alias_ptr, bits, false)
                .is_ok()
        });
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(alias_ptr, bits) });
        assert_eq!(
            bridge
                .handle_shard(bits)
                .lock()
                .raw_py
                .get(&bits)
                .map(|binding| &binding.address),
            Some(&canonical_addr)
        );
        assert!(
            !bridge
                .address_shard(alias_addr)
                .lock()
                .from_py
                .contains_key(&alias_addr)
        );
        assert!(!bridge.pyobj_matches_handle(alias_ptr, bits));
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(canonical_ptr, bits) });
    }

    #[test]
    fn static_binding_unbind_isolated_from_other_identity_classes() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let mut first = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let mut second = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let first_ptr = &raw mut first;
        let second_ptr = &raw mut second;
        let first_bits = 0xA110_0000_0000_0060;
        let second_bits = 0xA110_0000_0000_0070;
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(first_ptr, first_bits, true)
                .is_ok()
        });
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(second_ptr, second_bits, true)
                .is_ok()
        });
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(first_ptr, first_bits) });
        assert_eq!(
            bridge
                .address_shard(second_ptr.addr())
                .lock()
                .direct_molt_py
                .get(&second_ptr.addr()),
            Some(&second_bits)
        );

        let managed_bits = MoltObject::from_int(20_001).bits();
        let managed_ptr = unsafe { bridge.owned_handle_to_pyobj(managed_bits) };
        assert!(!unsafe {
            bridge.unbind_static_pyobj_from_runtime_handle(managed_ptr, managed_bits)
        });
        assert_eq!(
            bridge
                .pyobj_to_handle(managed_ptr)
                .map(BridgeIdentity::as_handle),
            Some(managed_bits)
        );

        let mut foreign = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let foreign_ptr = &raw mut foreign;
        let foreign_bits = 0xA110_0000_0000_0080;
        bridge.insert_foreign_for_test(foreign_ptr, foreign_bits);
        assert!(!unsafe {
            bridge.unbind_static_pyobj_from_runtime_handle(foreign_ptr, foreign_bits)
        });
        assert_eq!(
            bridge
                .address_shard(foreign_ptr.addr())
                .lock()
                .foreign
                .get(&foreign_ptr.addr()),
            Some(&foreign_bits)
        );

        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(second_ptr, second_bits) });
        assert_eq!(
            bridge.release_pyobj(managed_ptr),
            PyObjRelease::ManagedViewRetired
        );
        unsafe { bridge.release_foreign(foreign_ptr.addr()) };
    }
}
