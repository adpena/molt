//! Managed C references, projection accounting, retirement and GC transitions.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;

use super::identity::pyobj_to_handle_static;
use super::publication::{is_publication_owner, observe_publication};

pub(super) fn release_bridge_entry(mut entry: BridgeEntry) {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let _ = unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(entry.bits, 0) };
    entry.view.release_owned_items();
}

/// A canonical projection removed from both bridge identity maps but kept
/// alive until the runtime has published every semantic edge source empty.
pub struct RetiredRuntimeView {
    entry: Option<Box<BridgeEntry>>,
}

impl Drop for RetiredRuntimeView {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            release_bridge_entry(*entry);
        }
    }
}

/// The mutable Type mirrors corresponding to runtime class cycle-clear slots.
/// Identity stays published: native and other C owners retire by ordinary RC.
pub struct RetiredTypeCycleProjection {
    pointers: [*mut PyObject; 2],
}

impl Drop for RetiredTypeCycleProjection {
    fn drop(&mut self) {
        crate::api::errors::with_preserved_error(|| {
            for pointer in self.pointers {
                unsafe { GLOBAL_BRIDGE.projection_decref(pointer) };
            }
        });
    }
}

/// Ordinary C-owned fields removed from a fixed physical projection. Exception
/// fields and CFunction.m_module share this deferred retirement authority;
/// private slot mirrors and list shadow ownership use their own protocols.
/// Publication is complete before construction, and no bridge lock may remain
/// held when the guard releases references and invokes arbitrary finalizers.
pub struct RetiredOwnedCFields {
    pointers: [*mut PyObject; EXCEPTION_VIEW_POINTER_FIELDS],
}

impl RetiredOwnedCFields {
    pub(super) fn one(pointer: *mut PyObject) -> Self {
        let mut pointers = [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS];
        pointers[0] = pointer;
        Self { pointers }
    }
}

impl From<ExceptionViewState> for RetiredOwnedCFields {
    fn from(state: ExceptionViewState) -> Self {
        Self {
            pointers: state.pointers(),
        }
    }
}

impl Drop for RetiredOwnedCFields {
    fn drop(&mut self) {
        let pointers = std::mem::replace(
            &mut self.pointers,
            [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS],
        );
        unsafe { crate::api::errors::release_preserving_error(&pointers) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BridgeLifecycle {
    /// Only the canonical view's stable runtime hold remains. `ob_refcnt`
    /// contains direct C references only.
    ViewHoldOnly,
    /// At least one non-view runtime owner exists. `ob_refcnt` includes one
    /// borrowed-view bias in addition to direct C references.
    RuntimeOwned,
    /// The stable view hold and a distinct runtime finalizer pin are live.
    /// `ob_refcnt` includes the matching finalizer bias. Ordinary runtime-owner
    /// 1<->2 transitions are suppressed until the window resolves.
    FinalizingPin,
}

/// Result of one header release linearized with its canonical ABI-view
/// lifecycle. `should_finalize` means the bridge has already published the
/// `FinalizingPin` gate, so no later non-owning runtime upgrade can reopen the
/// view-hold-only baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeOwnerRelease {
    previous: u32,
    should_finalize: bool,
}

impl RuntimeOwnerRelease {
    #[inline]
    pub const fn previous(self) -> u32 {
        self.previous
    }

    #[inline]
    pub const fn should_finalize(self) -> bool {
        self.should_finalize
    }
}

impl BridgeLifecycle {
    #[inline]
    pub(super) fn has_c_bias(self) -> bool {
        matches!(self, Self::RuntimeOwned | Self::FinalizingPin)
    }
}

/// Project runtime ownership into the initial C header. Shape and physical
/// storage do not determine lifetime: an empty tuple subclass is mortal, while
/// canonical strings and tuples inherit their runtime owner's immortality.
#[inline]
pub(super) fn initial_managed_view_refs(runtime_refs: usize, owned: bool) -> (isize, bool) {
    let has_runtime_owner = !owned || runtime_refs > 1;
    let c_refs = if runtime_refs == molt_codegen_abi::IMMORTAL_REFCOUNT as usize {
        crate::abi_types::IMMORTAL_REFCNT
    } else {
        isize::from(owned) + isize::from(has_runtime_owner)
    };
    (c_refs, has_runtime_owner)
}

/// C immortality is a lifetime state, not a large count from which a runtime
/// bias or mirrored edge may be subtracted. C API promotion and runtime-owned
/// canonical objects share this interpretation of the physical header.
#[derive(Clone, Copy)]
pub(super) enum CReferenceCount {
    Counted(isize),
    Immortal,
}

impl CReferenceCount {
    pub(super) fn read(refs: isize, operation: &str, lifecycle: BridgeLifecycle) -> Self {
        if refs < 0 {
            abort_refcount_invariant(operation, refs, lifecycle);
        }
        if crate::abi_types::is_immortal_refcnt(refs) {
            Self::Immortal
        } else {
            Self::Counted(refs)
        }
    }

    pub(super) fn without_bias(
        self,
        has_bias: bool,
        operation: &str,
        lifecycle: BridgeLifecycle,
    ) -> Self {
        match self {
            Self::Immortal => Self::Immortal,
            Self::Counted(refs) => Self::Counted(
                checked_c_refs_without_bias(refs, has_bias)
                    .unwrap_or_else(|| abort_refcount_invariant(operation, refs, lifecycle)),
            ),
        }
    }
}

impl BridgeEntry {
    pub(super) fn change_c_bias(&self, add: bool, operation: &str) {
        let ptr = self.view.py_obj();
        let refs = unsafe { (*ptr).ob_refcnt };
        if let CReferenceCount::Counted(refs) =
            CReferenceCount::read(refs, operation, self.lifecycle)
        {
            let updated = if add {
                checked_c_ref_increment(refs)
            } else {
                checked_c_refs_without_bias(refs, true)
            }
            .unwrap_or_else(|| abort_refcount_invariant(operation, refs, self.lifecycle));
            unsafe { (*ptr).ob_refcnt = updated };
        }
    }
}

#[inline]
fn checked_c_refs_without_bias(refs: isize, has_bias: bool) -> Option<isize> {
    if refs < 0 || crate::abi_types::is_immortal_refcnt(refs) {
        return None;
    }
    refs.checked_sub(isize::from(has_bias))
        .filter(|direct| *direct >= 0)
}

#[inline]
pub(super) fn checked_c_ref_increment(refs: isize) -> Option<isize> {
    if refs < 0 || crate::abi_types::is_immortal_refcnt(refs) {
        None
    } else {
        refs.checked_add(1)
    }
}

#[cold]
pub(super) fn abort_refcount_invariant(
    operation: &str,
    refs: isize,
    lifecycle: BridgeLifecycle,
) -> ! {
    eprintln!(
        "molt fatal: canonical ABI refcount invariant failed during {operation}: refs={refs} lifecycle={lifecycle:?}"
    );
    std::process::abort()
}

/// Storage authority selected when a C-visible object reaches its release
/// boundary.  This is deliberately not a boolean: managed views, registered
/// direct objects, numeric carriers, immortal statics, and unknown foreign
/// objects have distinct destruction obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyObjRelease {
    ManagedViewRetired,
    DirectViewUnregistered,
    NumericCarrier,
    StaticImmortal,
    Untracked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedDecref {
    NotManaged,
    Immortal,
    Alive,
    RetiredInline,
    ReleaseRuntimeHold(AbiHandle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CRefZero {
    ViewRetained,
    ReleaseRuntimeHold,
}

impl PyObjRelease {
    #[inline]
    pub const fn requires_type_dealloc(self) -> bool {
        matches!(
            self,
            Self::DirectViewUnregistered | Self::NumericCarrier | Self::Untracked
        )
    }
}

impl ObjectBridge {
    /// Detach mutable Type mirrors after their semantic slots have become empty.
    /// Keep identity published and release the returned owners outside locks.
    pub fn clear_type_view_cycle_edges(
        &self,
        bits: AbiHandle,
    ) -> Option<RetiredTypeCycleProjection> {
        let mut handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get_mut(&bits)?;
        if !matches!(entry.view, ManagedView::Type { .. }) {
            return None;
        }
        let mut pointers = [std::ptr::null_mut(); 2];
        let mut count = 0;
        entry
            .view
            .owned_items_with(true, true, |pointer, mirrored| {
                assert!(mirrored);
                if !pointer.is_null() {
                    pointers[count] = pointer;
                    count += 1;
                }
            });
        Some(RetiredTypeCycleProjection { pointers })
    }

    /// Detach ordinary C-owned physical fields after the runtime payload has
    /// published its cleared state. The returned guard owns every displaced
    /// reference; callbacks run only when the caller releases detached resources
    /// outside bridge locks. Slot mirrors retain their terminal release order.
    /// Null publication makes repeated clear and terminal teardown idempotent.
    pub fn clear_owned_c_fields(&self, bits: AbiHandle) -> Option<RetiredOwnedCFields> {
        let old = {
            let mut handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get_mut(&bits)?;
            match &mut entry.view {
                ManagedView::Exception(allocation) => {
                    let empty = ExceptionViewState::empty(allocation.layout_kind());
                    unsafe { allocation.replace_state(empty)? }.pointers()
                }
                ManagedView::CFunction(object) => {
                    let mut pointers = [std::ptr::null_mut(); EXCEPTION_VIEW_POINTER_FIELDS];
                    pointers[0] = unsafe {
                        std::mem::replace(&mut (*object.get()).func.m_module, std::ptr::null_mut())
                    };
                    pointers
                }
                _ => return None,
            }
        };
        Some(RetiredOwnedCFields { pointers: old })
    }
}

// Projection-owned C-reference accounting and numeric carrier retirement.
impl ObjectBridge {
    /// Private C mirrors already represented by runtime graph ownership.
    /// Managed and native GC nodes discount this same ledger; independently
    /// traversed C fields never enter it.
    pub fn mirrored_c_refcount(&self, address: usize) -> usize {
        self.address_shard(address)
            .lock()
            .projection_refs
            .get(&address)
            .copied()
            .unwrap_or(0)
    }

    /// Retain a private mirror already represented in the runtime ownership
    /// graph. C-writable fields (exception members and CFunction.m_module) own
    /// ordinary C references and are visited independently by shared cycle GC.
    pub(super) unsafe fn projection_incref(&self, ptr: *mut PyObject) -> bool {
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            return true;
        }
        if !unsafe { self.projection_adopt_owned_ref(ptr) } {
            return false;
        }
        unsafe { crate::api::refcount::Py_INCREF(ptr) };
        true
    }

    /// Adopt an already-owned private mirror without incrementing it again.
    pub(super) unsafe fn projection_adopt_owned_ref(&self, ptr: *mut PyObject) -> bool {
        if ptr.is_null() || unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            return true;
        }
        let mut address = self.address_shard(ptr.addr()).lock();
        let count = address
            .projection_refs
            .get(&ptr.addr())
            .copied()
            .unwrap_or(0);
        let Some(next) = count.checked_add(1) else {
            drop(address);
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        };
        if count == 0 && address.projection_refs.try_reserve(1).is_err() {
            drop(address);
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return false;
        }
        address.projection_refs.insert(ptr.addr(), next);
        true
    }

    /// Undo adoption without consuming the C reference during staging rollback.
    pub(super) unsafe fn projection_unadopt_owned_ref(&self, ptr: *mut PyObject) {
        if ptr.is_null() {
            return;
        }
        let mut address = self.address_shard(ptr.addr()).lock();
        if unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
            // Promotion can occur after these mirrors were retained. Their
            // numeric ownership is now absorbed by the process-long C root.
            address.projection_refs.remove(&ptr.addr());
            return;
        }
        let remove = match address.projection_refs.get_mut(&ptr.addr()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                *count == 0
            }
            _ => {
                eprintln!(
                    "molt fatal: missing or invalid mirrored projection reference ledger entry"
                );
                std::process::abort();
            }
        };
        if remove {
            address.projection_refs.remove(&ptr.addr());
        }
    }

    /// Release one projection-owned C edge. Publish the ledger decrement before
    /// Py_DECREF can enter terminal/refcount-zero logic.
    pub(super) unsafe fn projection_decref(&self, ptr: *mut PyObject) {
        if ptr.is_null() {
            return;
        }
        unsafe { self.projection_unadopt_owned_ref(ptr) };
        unsafe { crate::api::refcount::Py_DECREF(ptr) };
    }

    pub(crate) fn unregister_numeric_carrier(
        &self,
        ptr: *mut PyObject,
    ) -> Option<NumericCarrierRecord> {
        if ptr.is_null() {
            return None;
        }
        self.address_shard(ptr.addr())
            .lock()
            .numeric_carriers
            .remove(&ptr.addr())
    }
}

// Managed-view C-reference linearization, publication retirement, and view removal.
impl ObjectBridge {
    /// A managed immortal C view retains its stable runtime hold as a permanent
    /// external root. GC membership admission must preserve that lifetime after
    /// container mutation, without inspecting C layout from the runtime.
    pub fn is_immortal_c_view(&self, bits: AbiHandle) -> bool {
        let handle = self.handle_shard(bits).lock();
        handle.to_py.get(&bits).is_some_and(|entry| unsafe {
            crate::abi_types::is_immortal_refcnt((*entry.view.py_obj()).ob_refcnt)
        })
    }

    pub(super) fn managed_entry_is_unique(&self, entry: &BridgeEntry, mirrored: usize) -> bool {
        if entry.lifecycle == BridgeLifecycle::FinalizingPin {
            return false;
        }
        let refs = unsafe { (*entry.view.py_obj()).ob_refcnt };
        let CReferenceCount::Counted(c_refs) =
            CReferenceCount::read(refs, "unique ownership query", entry.lifecycle).without_bias(
                entry.lifecycle.has_c_bias(),
                "unique ownership query",
                entry.lifecycle,
            )
        else {
            return false;
        };
        let runtime_owners = if MoltObject::from_bits(entry.bits).as_ptr().is_some() {
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(entry.bits) };
            if runtime_refs == molt_codegen_abi::IMMORTAL_REFCOUNT as usize {
                return false;
            }
            runtime_refs.checked_sub(1).unwrap_or_else(|| {
                abort_refcount_invariant("unique ownership runtime hold", refs, entry.lifecycle)
            })
        } else {
            0
        };
        let direct_refs = usize::try_from(c_refs)
            .ok()
            .and_then(|refs| refs.checked_sub(mirrored))
            .unwrap_or_else(|| {
                abort_refcount_invariant("unique ownership mirrors", refs, entry.lifecycle)
            });
        runtime_owners.checked_add(direct_refs) == Some(1)
    }

    /// Observe semantic ownership while excluding the bridge hold and duplicate
    /// private mirrors. A borrowed C bias does not imply unique runtime custody.
    pub unsafe fn managed_is_uniquely_referenced(&self, ptr: *mut PyObject) -> Option<bool> {
        let bits = self.managed_handle_for_pyobj(ptr)?;
        let (address, handle) = self.lock_address_then_handle(ptr.addr(), bits);
        let entry = handle.to_py.get(&bits)?;
        Some(
            self.managed_entry_is_unique(
                entry,
                address
                    .projection_refs
                    .get(&ptr.addr())
                    .copied()
                    .unwrap_or(0),
            ),
        )
    }

    /// Mirror-adjusted ownership for a live runtime handle. None positively
    /// identifies absence of a managed view; a changed/finalizing view refuses.
    /// The runtime caller holds its ordinary GIL/object lifetime custody.
    pub fn managed_handle_is_uniquely_referenced(&self, bits: AbiHandle) -> Option<bool> {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (address, handle) = self.lock_address_then_handle(addr, bits);
        let Some(entry) = handle.to_py.get(&bits) else {
            return Some(false);
        };
        if entry.view.py_obj().addr() != addr {
            return Some(false);
        }
        Some(self.managed_entry_is_unique(
            entry,
            address.projection_refs.get(&addr).copied().unwrap_or(0),
        ))
    }

    /// Check and promote in one transaction. The stable runtime hold becomes
    /// the external lifetime root; no shared runtime header is immortalized.
    pub unsafe fn managed_try_set_immortal(&self, ptr: *mut PyObject) -> Option<bool> {
        let bits = self.managed_handle_for_pyobj(ptr)?;
        let (mut address, handle) = self.lock_address_then_handle(ptr.addr(), bits);
        let entry = handle.to_py.get(&bits)?;
        if !self.managed_entry_is_unique(
            entry,
            address
                .projection_refs
                .get(&ptr.addr())
                .copied()
                .unwrap_or(0),
        ) {
            return Some(false);
        }
        unsafe { (*ptr).ob_refcnt = crate::abi_types::IMMORTAL_REFCNT };
        address.projection_refs.remove(&ptr.addr());
        Some(true)
    }

    /// Canonical header mutation for Py_SET_REFCNT. Immortality is one-way;
    /// promotion absorbs all numeric private mirrors into the lifetime root.
    pub unsafe fn set_pyobj_refcnt(&self, ptr: *mut PyObject, refs: isize) {
        if ptr.is_null() {
            return;
        }
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        if let Some(bits) = self.managed_handle_for_pyobj(ptr) {
            let (mut address, handle) = self.lock_address_then_handle(ptr.addr(), bits);
            let entry = handle
                .to_py
                .get(&bits)
                .expect("owned refcount mutation lost its view");
            if unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
                return;
            }
            let ownership = CReferenceCount::read(refs, "Py_SET_REFCNT", entry.lifecycle);
            unsafe { (*ptr).ob_refcnt = refs };
            if matches!(ownership, CReferenceCount::Immortal) {
                address.projection_refs.remove(&ptr.addr());
            }
        } else {
            let mut address = self.address_shard(ptr.addr()).lock();
            if !unsafe { crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) } {
                unsafe { (*ptr).ob_refcnt = refs };
                if crate::abi_types::is_immortal_refcnt(refs) {
                    address.projection_refs.remove(&ptr.addr());
                }
            }
        }
    }

    /// Increment a canonical managed view while holding both identity ranks.
    /// The caller must hold the runtime execution token; the bridge lock makes
    /// the C header update linearizable with publication and retirement.
    pub unsafe fn managed_incref_pyobj(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        let addr = ptr.addr();
        loop {
            let address = self.address_shard(addr).lock();
            let bits = address.from_py.get(&addr).copied()?;
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let entry = handle.to_py.get_mut(&bits)?;
            match &entry.publication {
                PublicationState::Ready => {}
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                    if !observe_publication(self, bits) {
                        // Py_INCREF cannot report a recoverable failure. Never
                        // let an escaped rollback skeleton gain a reference
                        // which retirement cannot honor.
                        abort_refcount_invariant(
                            "Py_INCREF of an aborting publication",
                            unsafe { (*ptr).ob_refcnt },
                            entry.lifecycle,
                        );
                    }
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                    continue;
                }
            }
            if entry.view.py_obj() != ptr {
                return None;
            }
            let refs = unsafe { (*ptr).ob_refcnt };
            if crate::abi_types::is_immortal_refcnt(refs) {
                return Some(bits);
            }
            let Some(incremented) = checked_c_ref_increment(refs) else {
                abort_refcount_invariant("managed Py_INCREF", refs, entry.lifecycle);
            };
            unsafe { (*ptr).ob_refcnt = incremented };
            return Some(bits);
        }
    }

    /// Decrement a canonical managed view as one identity/refcount/lifecycle
    /// transaction.  Heap terminal release remains owned by the runtime;
    /// immediate values retire synchronously because they have no finalizer.
    pub unsafe fn managed_decref_pyobj(&self, ptr: *mut PyObject) -> ManagedDecref {
        let addr = ptr.addr();
        loop {
            let mut address = self.address_shard(addr).lock();
            let Some(bits) = address.from_py.get(&addr).copied() else {
                return ManagedDecref::NotManaged;
            };
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return ManagedDecref::NotManaged;
            };
            match &entry.publication {
                PublicationState::Ready => {}
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                    continue;
                }
            }
            if entry.view.py_obj() != ptr {
                return ManagedDecref::NotManaged;
            }
            let refs = unsafe { (*ptr).ob_refcnt };
            if crate::abi_types::is_immortal_refcnt(refs) {
                return ManagedDecref::Immortal;
            }
            if refs <= 0 {
                return ManagedDecref::Alive;
            }
            let remaining = refs - 1;
            unsafe { (*ptr).ob_refcnt = remaining };
            if remaining != 0 {
                return ManagedDecref::Alive;
            }
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                unsafe { (*ptr).ob_refcnt = 1 };
                return ManagedDecref::Alive;
            }
            if MoltObject::from_bits(bits).as_ptr().is_none() {
                entry.publication = PublicationState::Retiring;
                address.from_py.remove(&addr);
                address.direct_molt_py.remove(&addr);
                let entry = handle
                    .to_py
                    .remove(&bits)
                    .expect("inline managed view disappeared during decref");
                self.publication_ready[index].notify_all();
                drop(handle);
                drop(address);
                release_bridge_entry(*entry);
                return ManagedDecref::RetiredInline;
            }
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
            if runtime_refs > 1 {
                unsafe { (*ptr).ob_refcnt = 1 };
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
                return ManagedDecref::Alive;
            }
            // Publish the terminal handoff before returning to the runtime
            // header release. Checked non-owning upgrades lock this same entry
            // and must not slip between the C-zero verdict and that release.
            unsafe { (*ptr).ob_refcnt = 1 };
            entry.lifecycle = BridgeLifecycle::FinalizingPin;
            return ManagedDecref::ReleaseRuntimeHold(bits);
        }
    }

    pub fn release_pyobj(&self, ptr: *mut PyObject) -> PyObjRelease {
        if ptr.is_null() {
            return PyObjRelease::Untracked;
        }
        if pyobj_to_handle_static(ptr).is_some() {
            return PyObjRelease::StaticImmortal;
        }
        let addr = ptr.addr();
        loop {
            let mut address = self.address_shard(addr).lock();
            if address.numeric_carriers.contains_key(&addr) {
                return PyObjRelease::NumericCarrier;
            }
            let Some(bits) = address.from_py.get(&addr).copied() else {
                return PyObjRelease::Untracked;
            };
            let index = self.handle_shard_index(bits);
            let mut handle = self.handle_shards[index].lock();
            if handle.managed_address(bits) != Some(addr) {
                if address.direct_molt_py.get(&addr).copied() != Some(bits) {
                    return PyObjRelease::Untracked;
                }
                // Direct ingress owns its forward pair independently of any
                // reverse entry. A noncanonical alias may have no reverse, or
                // may share a handle whose canonical projection is managed.
                // Neither case grants authority to retire that other view.
                if handle
                    .raw_py
                    .get(&bits)
                    .is_some_and(|binding| binding.address == addr)
                {
                    handle.raw_py.remove(&bits);
                }
                address.direct_molt_py.remove(&addr);
                address.from_py.remove(&addr);
                return PyObjRelease::DirectViewUnregistered;
            }
            let entry = handle
                .to_py
                .get_mut(&bits)
                .expect("exact managed address lost its locked owner");
            match &entry.publication {
                PublicationState::Ready => entry.publication = PublicationState::Retiring,
                PublicationState::Building { owner } if is_publication_owner(self, bits, owner) => {
                    entry.publication = PublicationState::Retiring;
                }
                PublicationState::Building { .. } | PublicationState::Retiring => {
                    drop(address);
                    self.publication_ready[index].wait(&mut handle);
                    continue;
                }
            }
            address.direct_molt_py.remove(&addr);
            address.from_py.remove(&addr);
            let entry = handle
                .to_py
                .remove(&bits)
                .expect("managed publication disappeared during retirement");
            self.publication_ready[index].notify_all();
            drop(handle);
            drop(address);
            release_bridge_entry(*entry);
            unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
            return PyObjRelease::ManagedViewRetired;
        }
    }

    pub(super) fn remove_managed_view(&self, bits: AbiHandle, addr: usize) -> bool {
        let index = self.handle_shard_index(bits);
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        if let Some(entry) = handle.to_py.get_mut(&bits) {
            entry.publication = PublicationState::Retiring;
        }
        let entry = handle.to_py.remove(&bits);
        self.publication_ready[index].notify_all();
        drop(handle);
        drop(address);
        if let Some(entry) = entry {
            release_bridge_entry(*entry);
            true
        } else {
            false
        }
    }

    pub(super) fn retire_managed_view_entry_deferred(
        &self,
        bits: AbiHandle,
    ) -> Option<Box<BridgeEntry>> {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        if entry.view.py_obj().addr() != addr {
            return None;
        }
        // This is ordinary RC terminal removal, not forced interpreter
        // retirement. Physical C owners must have drained before rc-zero can
        // revoke identity. Runtime/collector pins never license skipping them.
        assert_eq!(
            address.projection_refs.get(&addr).copied().unwrap_or(0),
            0,
            "terminal view removal preceded incoming projection release"
        );
        entry.publication = PublicationState::Retiring;
        address.from_py.remove(&addr);
        address.direct_molt_py.remove(&addr);
        let entry = handle.to_py.remove(&bits)?;
        self.publication_ready[self.handle_shard_index(bits)].notify_all();
        drop(handle);
        drop(address);
        Some(entry)
    }

    /// Ordinary rc-zero retirement after direct C owners and incoming mirrors
    /// have drained. This includes TYPE_ID_TYPE; it cannot force a live Type
    /// out of an owning projection cycle. Interpreter cohort retirement uses
    /// retire_runtime_type_views; GC detaches mirrors and preserves identity.
    /// Physical outgoing edges wait until semantic sources are empty.
    pub fn retire_runtime_object_deferred(&self, bits: AbiHandle) -> Option<RetiredRuntimeView> {
        let entry = self.retire_managed_view_entry_deferred(bits)?;
        Some(RetiredRuntimeView { entry: Some(entry) })
    }

    /// Observe physical view presence without materializing an identity.
    pub fn has_managed_type_view(&self, bits: AbiHandle) -> bool {
        self.handle_shard(bits)
            .lock()
            .to_py
            .get(&bits)
            .is_some_and(|entry| matches!(entry.view, ManagedView::Type { .. }))
    }

    /// Retire the complete incoming projection graph of runtime type
    /// views. A type's MRO owns a C edge back to the type; any retained tuple,
    /// list or other concrete view can in turn own that MRO/type view. Runtime
    /// pins alone cannot keep those physical C allocations alive.
    ///
    /// The caller owns shutdown callback custody and pins the canonical class
    /// roots. Both runtime/C thread-state domains and native owners must have
    /// drained first: no saved exception or external guard may later restore
    /// or decref a selected pointer. This belongs in the outer callback fixed
    /// point before semantic class metadata is detached; releasing an alias's
    /// stable hold can run Python and requires another owner-drain pass.
    /// Existing publication transaction custody supplies the C pins, closed
    /// observations, edge drain, identity revocation and final runtime releases.
    /// No independent address registry or raw post-revocation decref is used.
    pub fn retire_runtime_type_views(&self, roots: &[AbiHandle]) -> bool {
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        let roots: std::collections::HashSet<_> = roots
            .iter()
            .copied()
            .filter(|&bits| self.has_managed_type_view(bits))
            .collect();
        if roots.is_empty() {
            return false;
        }
        struct Node {
            bits: AbiHandle,
            pointer: *mut PyObject,
            selected: bool,
        }
        let mut nodes = Vec::new();
        let mut incoming: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        let mut pending = Vec::new();
        for shard in self.handle_shards.iter() {
            let mut shard = shard.lock();
            for (&bits, entry) in &mut shard.to_py {
                let selected = roots.contains(&bits);
                let pointer = entry.view.py_obj();
                let owner = nodes.len();
                entry.view.owned_items_with(false, false, |edge, _| {
                    if !edge.is_null() {
                        incoming.entry(edge.addr()).or_default().push(owner);
                    }
                });
                if selected {
                    pending.push(owner);
                }
                nodes.push(Node {
                    bits,
                    pointer,
                    selected,
                });
            }
        }
        // Reverse closure includes every physical owner, including ordinary
        // C-writable fields as well as mirrors. Following only type -> MRO
        // would miss a retained alias or a sibling's bases/MRO projection.
        // One indexed worklist visits each node/edge once; no per-class scan.
        let mut cursor = 0;
        while cursor < pending.len() {
            let address = nodes[pending[cursor]].pointer.addr();
            cursor += 1;
            if let Some(owners) = incoming.get(&address) {
                for &owner in owners {
                    if !nodes[owner].selected {
                        nodes[owner].selected = true;
                        pending.push(owner);
                    }
                }
            }
        }
        drop(incoming);
        drop(pending);
        let component: Vec<_> = nodes.into_iter().filter(|node| node.selected).collect();
        self.retire_publication_component(
            component.into_iter().map(|node| (node.bits, node.pointer)),
        );
        true
    }
    /// Add one runtime owner while holding the canonical ABI-view lifecycle
    /// lock. The supplied header transition therefore cannot race the mirrored
    /// `RuntimeOwned <-> ViewHoldOnly` bias or the `FinalizingPin` terminal gate.
    /// Non-owning callers pass `allow_finalizing = false`; ordinary owned
    /// resurrection inside Python finalizer code passes `true`.
    pub fn transition_runtime_owner_add<F>(
        &self,
        bits: AbiHandle,
        allow_finalizing: bool,
        internal_pins: u32,
        retain: F,
    ) -> Option<u32>
    where
        F: FnOnce() -> Option<u32>,
    {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (_address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        if entry.lifecycle == BridgeLifecycle::FinalizingPin && !allow_finalizing {
            return None;
        }
        let previous = retain()?;
        let semantic_previous = previous.checked_sub(internal_pins).unwrap_or_else(|| {
            eprintln!(
                "molt fatal: ABI runtime-owner add underflow previous={previous} internal_pins={internal_pins}"
            );
            std::process::abort();
        });
        match (semantic_previous, entry.lifecycle) {
            (1, BridgeLifecycle::ViewHoldOnly) => {
                entry.change_c_bias(true, "runtime owner add");
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
            }
            (_, BridgeLifecycle::ViewHoldOnly) => {
                eprintln!(
                    "molt fatal: view-hold-only ABI lifecycle had multiple runtime owners before add"
                );
                std::process::abort();
            }
            (_, BridgeLifecycle::RuntimeOwned | BridgeLifecycle::FinalizingPin) => {}
        }
        Some(previous)
    }

    /// Release one runtime owner as the inverse transaction. A terminal result
    /// publishes `FinalizingPin` before either bridge lock is released; checked
    /// weakref retention consequently rejects the stable view baseline before
    /// the runtime exposes its internal revival pin.
    pub fn transition_runtime_owner_release<F, R>(
        &self,
        bits: AbiHandle,
        internal_pins: u32,
        release: F,
        restore_stable_view_hold: R,
    ) -> Option<RuntimeOwnerRelease>
    where
        F: FnOnce() -> u32,
        R: FnOnce(),
    {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            entry.view.py_obj().addr()
        };
        let (_address, mut handle) = self.lock_address_then_handle(addr, bits);
        let entry = handle.to_py.get_mut(&bits)?;
        let previous = release();
        let semantic_previous = previous.checked_sub(internal_pins).unwrap_or_else(|| {
            eprintln!(
                "molt fatal: ABI runtime-owner release underflow previous={previous} internal_pins={internal_pins}"
            );
            std::process::abort();
        });
        if semantic_previous > 2 {
            if entry.lifecycle == BridgeLifecycle::ViewHoldOnly {
                eprintln!("molt fatal: view-hold-only ABI lifecycle retained multiple owners");
                std::process::abort();
            }
            return Some(RuntimeOwnerRelease {
                previous,
                should_finalize: false,
            });
        }
        let py_obj = entry.view.py_obj();
        if semantic_previous == 2 {
            match entry.lifecycle {
                BridgeLifecycle::FinalizingPin => {
                    return Some(RuntimeOwnerRelease {
                        previous,
                        should_finalize: false,
                    });
                }
                BridgeLifecycle::ViewHoldOnly => {
                    eprintln!(
                        "molt fatal: canonical ABI view lost runtime-owner bias before owner drop"
                    );
                    std::process::abort();
                }
                BridgeLifecycle::RuntimeOwned => {}
            }
        } else {
            if semantic_previous != 1 {
                eprintln!(
                    "molt fatal: invalid stable-view runtime release previous={previous} internal_pins={internal_pins} lifecycle={:?}",
                    entry.lifecycle
                );
                std::process::abort();
            }
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                restore_stable_view_hold();
                return Some(RuntimeOwnerRelease {
                    previous,
                    should_finalize: true,
                });
            }
        }
        if entry.lifecycle == BridgeLifecycle::RuntimeOwned {
            entry.change_c_bias(false, "runtime owner drop");
            entry.lifecycle = BridgeLifecycle::ViewHoldOnly;
        }
        if semantic_previous == 1 {
            restore_stable_view_hold();
        }
        if unsafe { (*py_obj).ob_refcnt } != 0 {
            return Some(RuntimeOwnerRelease {
                previous,
                should_finalize: false,
            });
        }
        unsafe { (*py_obj).ob_refcnt = 1 };
        entry.lifecycle = BridgeLifecycle::FinalizingPin;
        Some(RuntimeOwnerRelease {
            previous,
            should_finalize: true,
        })
    }

    /// Rebase an immortal managed view onto the ordinary runtime-owner bias
    /// before its immortal runtime object enters the shutdown-only mortal lane.
    ///
    /// CPython reference operations on an immortal view are deliberately
    /// no-ops, so there are no countable direct C owners to preserve at this
    /// boundary. Runtime shutdown has already stopped new execution entrants;
    /// it may therefore restore the single encoded owner bias that the normal
    /// `RuntimeOwned -> ViewHoldOnly` terminal transition consumes.
    pub fn prepare_runtime_immortal_for_shutdown(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        let py_obj = entry.view.py_obj();
        if !unsafe { crate::abi_types::is_immortal_refcnt((*py_obj).ob_refcnt) } {
            return;
        }
        if entry.lifecycle != BridgeLifecycle::RuntimeOwned {
            eprintln!(
                "molt fatal: immortal canonical ABI view entered shutdown outside RuntimeOwned: {:?}",
                entry.lifecycle
            );
            std::process::abort();
        }
        unsafe { (*py_obj).ob_refcnt = 1 };
    }

    /// Direct C references created during a finalizer/weakref revival window
    /// are resurrection roots even though they do not change runtime RC.
    pub fn has_direct_c_refs(&self, bits: AbiHandle) -> bool {
        let (ptr, refs, lifecycle) = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return false;
            };
            (
                entry.view.py_obj(),
                unsafe { (*entry.view.py_obj()).ob_refcnt },
                entry.lifecycle,
            )
        };
        let CReferenceCount::Counted(direct_and_projection) =
            CReferenceCount::read(refs, "direct C reference query", lifecycle).without_bias(
                lifecycle.has_c_bias(),
                "direct C reference query",
                lifecycle,
            )
        else {
            return true;
        };
        let projection = self.mirrored_c_refcount(ptr.addr());
        let Ok(projection) = isize::try_from(projection) else {
            abort_refcount_invariant("projection reference query", refs, lifecycle);
        };
        if projection > direct_and_projection {
            abort_refcount_invariant("projection reference query", refs, lifecycle);
        }
        direct_and_projection != projection
    }

    pub fn begin_finalization(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        if entry.lifecycle == BridgeLifecycle::FinalizingPin {
            if unsafe { (*entry.view.py_obj()).ob_refcnt } < 1 {
                eprintln!("molt fatal: canonical ABI view lost its published finalizing pin");
                std::process::abort();
            }
            return;
        }
        if entry.lifecycle != BridgeLifecycle::ViewHoldOnly {
            eprintln!(
                "molt fatal: canonical ABI view finalization began outside ViewHoldOnly: {:?}",
                entry.lifecycle
            );
            std::process::abort();
        }
        let py_obj = entry.view.py_obj();
        if unsafe { (*py_obj).ob_refcnt } != 0 {
            eprintln!("molt fatal: finalization began with unmatched direct C references");
            std::process::abort();
        }
        unsafe { (*py_obj).ob_refcnt = 1 };
        entry.lifecycle = BridgeLifecycle::FinalizingPin;
    }

    /// Adopt a canonical view first published by arbitrary finalizer code into
    /// the already-open runtime revival window. The publication's runtime-owner
    /// bias becomes the finalization pin in place: the stable runtime view hold
    /// remains part of the runtime baseline and any additional direct C roots
    /// remain visible above this pin.
    pub fn begin_finalization_for_new_view(&self, bits: AbiHandle) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            eprintln!("molt fatal: finalizer-published ABI view disappeared");
            std::process::abort();
        };
        if entry.lifecycle != BridgeLifecycle::RuntimeOwned {
            eprintln!(
                "molt fatal: finalizer-published ABI view was not runtime-owned: {:?}",
                entry.lifecycle
            );
            std::process::abort();
        }
        if unsafe { (*entry.view.py_obj()).ob_refcnt } < 1 {
            eprintln!("molt fatal: finalizer-published ABI view lost its runtime bias");
            std::process::abort();
        }
        entry.lifecycle = BridgeLifecycle::FinalizingPin;
    }

    pub fn finish_finalization(&self, bits: AbiHandle, runtime_resurrected: bool) {
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return;
        };
        if entry.lifecycle != BridgeLifecycle::FinalizingPin {
            eprintln!("molt fatal: canonical ABI view finalization state was lost");
            std::process::abort();
        }
        if runtime_resurrected {
            entry.lifecycle = BridgeLifecycle::RuntimeOwned;
        } else {
            entry.change_c_bias(false, "finalization pin release");
            entry.lifecycle = BridgeLifecycle::ViewHoldOnly;
        }
    }

    /// Retire a canonical view only after the runtime has passed every
    /// finalizer/weakref resurrection check and is committed to freeing the
    /// object. This preserves pointer identity throughout the revival window.
    pub fn runtime_object_destroyed(&self, bits: AbiHandle) {
        let addr = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return;
            };
            entry.view.py_obj().addr()
        };
        self.remove_managed_view(bits, addr);
    }
}

// Finalization completion, resurrection, GC roots, and zero-ref disposition.
impl ObjectBridge {
    /// Remove the stable view hold and private runtime-mirrored C references
    /// from the cycle collector's scratch count. C-writable physical fields
    /// contribute ordinary C references; their owners independently traverse
    /// their current pointers, including uncommitted direct writes.
    pub fn gc_ref_adjustment(&self, bits: AbiHandle) -> isize {
        let (ptr, c_refs, lifecycle) = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return 0;
            };
            (
                entry.view.py_obj(),
                unsafe { (*entry.view.py_obj()).ob_refcnt },
                entry.lifecycle,
            )
        };
        let CReferenceCount::Counted(direct_and_projection) =
            CReferenceCount::read(c_refs, "GC external-root adjustment", lifecycle).without_bias(
                lifecycle.has_c_bias(),
                "GC external-root adjustment",
                lifecycle,
            )
        else {
            // The stable runtime hold is an external root for the process-long
            // C lifetime. Neither that hold nor old mirrors are discounted.
            return 0;
        };
        let projection = self.mirrored_c_refcount(ptr.addr());
        let Ok(projection) = isize::try_from(projection) else {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        };
        let Some(c_owned_refs) = direct_and_projection.checked_sub(projection) else {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        };
        if c_owned_refs < 0 {
            abort_refcount_invariant("GC projection adjustment", c_refs, lifecycle);
        }
        c_owned_refs - 1
    }

    /// Finalization is an explicit GC root until the runtime pin is resolved.
    /// This is intentionally queried by the collector instead of inferred from
    /// a transient refcount/bias arithmetic coincidence.
    pub fn has_finalizing_pin(&self, bits: AbiHandle) -> bool {
        let handle = self.handle_shard(bits).lock();
        handle
            .to_py
            .get(&bits)
            .is_some_and(|entry| entry.lifecycle == BridgeLifecycle::FinalizingPin)
    }

    /// Handle direct CPython refcount reaching zero. Immediate scalar handles
    /// have no runtime allocation, finalizer, or resurrection window, so their
    /// canonical view is retired here. For heap handles, if other runtime
    /// owners remain, re-establish their borrowed-view bias. Otherwise keep the
    /// canonical view attached and tell `_Py_Dealloc` to release its runtime
    /// hold: the runtime terminal path owns finalization and retires identity
    /// only after the resurrection window closes.
    pub fn c_ref_zero(&self, bits: AbiHandle) -> CRefZero {
        if MoltObject::from_bits(bits).as_ptr().is_none() {
            let addr = {
                let handle = self.handle_shard(bits).lock();
                let Some(entry) = handle.to_py.get(&bits) else {
                    return CRefZero::ViewRetained;
                };
                entry.view.py_obj().addr()
            };
            self.remove_managed_view(bits, addr);
            return CRefZero::ViewRetained;
        }
        {
            let mut handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get_mut(&bits) else {
                return CRefZero::ViewRetained;
            };
            if entry.lifecycle == BridgeLifecycle::FinalizingPin {
                unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
                return CRefZero::ViewRetained;
            }
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
            if runtime_refs > 1 {
                unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
                entry.lifecycle = BridgeLifecycle::RuntimeOwned;
                return CRefZero::ViewRetained;
            }
            unsafe { (*entry.view.py_obj()).ob_refcnt = 1 };
            entry.lifecycle = BridgeLifecycle::FinalizingPin;
        }
        CRefZero::ReleaseRuntimeHold
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;
    #[test]
    fn managed_incref_decref_race_is_linearized_by_bridge_lock() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(50_000).bits();
        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
        let addr = ptr.addr();
        let workers = 8;
        let barrier = Arc::new(Barrier::new(workers));
        let (tx, rx) = mpsc::channel();
        for _ in 0..workers {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            thread::spawn(move || {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
                barrier.wait();
                for _ in 0..5_000 {
                    assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
                    assert_eq!(
                        unsafe { bridge.managed_decref_pyobj(ptr) },
                        ManagedDecref::Alive
                    );
                }
                tx.send(()).unwrap();
            });
        }
        for _ in 0..workers {
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn managed_immortal_view_refcount_operations_are_noops() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_000).bits();
        assert!(!bridge.is_immortal_c_view(bits));
        let ptr = unsafe { bridge.owned_handle_to_pyobj(bits) };
        assert!(!bridge.is_immortal_c_view(bits));
        assert!(unsafe { bridge.projection_incref(ptr) });
        assert_eq!(bridge.mirrored_c_refcount(ptr.addr()), 1);
        unsafe { bridge.set_pyobj_refcnt(ptr, crate::abi_types::IMMORTAL_REFCNT) };
        assert!(bridge.is_immortal_c_view(bits));
        assert_eq!(bridge.mirrored_c_refcount(ptr.addr()), 0);
        unsafe { bridge.set_pyobj_refcnt(ptr, 1) };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        assert_eq!(
            unsafe { bridge.managed_decref_pyobj(ptr) },
            ManagedDecref::Immortal
        );
        assert_eq!(
            unsafe { (*ptr).ob_refcnt },
            crate::abi_types::IMMORTAL_REFCNT
        );
        assert!(bridge.has_direct_c_refs(bits));
        assert_eq!(bridge.gc_ref_adjustment(bits), 0);
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, true, 0, || Some(1)),
            Some(1)
        );
        let release = bridge
            .transition_runtime_owner_release(bits, 0, || 2, || unreachable!())
            .expect("release a runtime owner while the immortal C root remains");
        assert!(!release.should_finalize());
        assert_eq!(
            unsafe { (*ptr).ob_refcnt },
            crate::abi_types::IMMORTAL_REFCNT
        );
        unsafe { bridge.projection_decref(ptr) };
        assert_eq!(bridge.mirrored_c_refcount(ptr.addr()), 0);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
        assert!(!bridge.is_immortal_c_view(bits));
    }

    #[test]
    fn immortal_runtime_view_rebases_once_for_terminal_shutdown() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_001).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        unsafe { (*ptr).ob_refcnt = crate::abi_types::IMMORTAL_REFCNT };

        bridge.prepare_runtime_immortal_for_shutdown(bits);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
        let release = bridge
            .transition_runtime_owner_release(bits, 0, || 1, || {})
            .expect("immortal shutdown owner release");
        assert!(release.should_finalize());
        bridge.begin_finalization(bits);
        bridge.finish_finalization(bits, false);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 0);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn runtime_owner_transition_publishes_terminal_gate_before_unlock() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_002).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        let runtime_refs = AtomicU32::new(2);

        let release = bridge
            .transition_runtime_owner_release(
                bits,
                0,
                || runtime_refs.fetch_sub(1, Ordering::AcqRel),
                || panic!("stable view hold must not be restored for 2 -> 1"),
            )
            .expect("runtime owner release");
        assert_eq!(release.previous(), 2);
        assert!(release.should_finalize());
        assert_eq!(runtime_refs.load(Ordering::Acquire), 1);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);

        let retain_called = std::cell::Cell::new(false);
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, false, 0, || {
                retain_called.set(true);
                Some(runtime_refs.fetch_add(1, Ordering::AcqRel))
            }),
            None,
        );
        assert!(!retain_called.get());
        bridge.begin_finalization(bits);
        bridge.finish_finalization(bits, false);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 0);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn runtime_owner_add_and_drop_update_view_bias_in_one_transaction() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_003).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        let runtime_refs = AtomicU32::new(2);

        let release = bridge
            .transition_runtime_owner_release(
                bits,
                0,
                || runtime_refs.fetch_sub(1, Ordering::AcqRel),
                || panic!("stable view hold must not be restored for 2 -> 1"),
            )
            .expect("runtime owner release");
        assert!(!release.should_finalize());
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, false, 0, || {
                Some(runtime_refs.fetch_add(1, Ordering::AcqRel))
            }),
            Some(1),
        );
        assert_eq!(runtime_refs.load(Ordering::Acquire), 2);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 2);
    }

    #[test]
    fn runtime_owner_transition_excludes_internal_gc_pin_from_liveness() {
        init_tag_table();
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(60_005).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        assert_eq!(unsafe { bridge.managed_incref_pyobj(ptr) }, Some(bits));
        let runtime_refs = AtomicU32::new(2);
        let release = bridge
            .transition_runtime_owner_release(
                bits,
                0,
                || runtime_refs.fetch_sub(1, Ordering::AcqRel),
                || panic!("stable view hold must not be restored for 2 -> 1"),
            )
            .expect("runtime owner release");
        assert!(!release.should_finalize());
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);

        runtime_refs.fetch_add(1, Ordering::AcqRel); // collector pin
        assert_eq!(
            bridge.transition_runtime_owner_add(bits, false, 1, || {
                Some(runtime_refs.fetch_add(1, Ordering::AcqRel))
            }),
            Some(2),
        );
        assert_eq!(runtime_refs.load(Ordering::Acquire), 3);
        assert_eq!(unsafe { (*ptr).ob_refcnt }, 2);
    }

    #[test]
    fn concurrent_runtime_owner_add_release_has_only_live_or_terminal_outcomes() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(60_004).bits();
        let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
        let runtime_refs = Arc::new(AtomicU32::new(2));
        let start = Arc::new(Barrier::new(2));

        let add_bridge = Arc::clone(&bridge);
        let add_refs = Arc::clone(&runtime_refs);
        let add_start = Arc::clone(&start);
        let add = thread::spawn(move || {
            add_start.wait();
            add_bridge.transition_runtime_owner_add(bits, false, 0, || {
                Some(add_refs.fetch_add(1, Ordering::AcqRel))
            })
        });
        let drop_bridge = Arc::clone(&bridge);
        let drop_refs = Arc::clone(&runtime_refs);
        let drop_start = Arc::clone(&start);
        let release = thread::spawn(move || {
            drop_start.wait();
            drop_bridge
                .transition_runtime_owner_release(
                    bits,
                    0,
                    || drop_refs.fetch_sub(1, Ordering::AcqRel),
                    || panic!("stable view hold must not be restored for this race"),
                )
                .expect("runtime owner release")
        });

        let added = add.join().expect("add worker");
        let released = release.join().expect("release worker");
        match runtime_refs.load(Ordering::Acquire) {
            1 => {
                assert_eq!(added, None);
                assert!(released.should_finalize());
                assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
            }
            2 => {
                assert_eq!(added, Some(2));
                assert_eq!(released.previous(), 3);
                assert!(!released.should_finalize());
                assert_eq!(unsafe { (*ptr).ob_refcnt }, 1);
            }
            refs => panic!("non-linearized runtime owner count {refs}"),
        }
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;
    #[test]
    fn lifecycle_refcount_arithmetic_rejects_corrupt_boundaries() {
        assert_eq!(checked_c_refs_without_bias(0, false), Some(0));
        assert_eq!(checked_c_refs_without_bias(1, true), Some(0));
        assert_eq!(checked_c_refs_without_bias(4, true), Some(3));
        assert_eq!(checked_c_refs_without_bias(0, true), None);
        assert_eq!(checked_c_refs_without_bias(-1, false), None);
        assert_eq!(checked_c_ref_increment(-1), None);
        assert_eq!(checked_c_ref_increment(isize::MAX), None);
        assert_eq!(checked_c_ref_increment(3), Some(4));
    }

    #[test]
    fn runtime_type_view_retirement_invalidates_only_managed_type_identity() {
        let bridge = ObjectBridge::new();
        let type_bits = MoltObject::from_int(31_001).bits();
        let mut type_object = unsafe { std::mem::zeroed::<PyTypeObject>() };
        type_object.ob_base.ob_base.ob_refcnt = 7;
        let type_view = ManagedView::Type {
            object: ManagedTypeAllocation::new(type_object),
            _name: std::ffi::CString::new("retiring.Type").unwrap(),
        };
        let type_ptr = type_view.py_obj();
        let type_addr = type_ptr.addr();
        let type_entry = Box::new(BridgeEntry {
            view: type_view,
            bits: type_bits,
            unicode: None,
            publication: PublicationState::Ready,
            lifecycle: BridgeLifecycle::RuntimeOwned,
        });
        {
            let (mut address, mut handle) = bridge.lock_address_then_handle(type_addr, type_bits);
            address.from_py.insert(type_addr, type_bits);
            address.direct_molt_py.insert(type_addr, type_bits);
            handle.to_py.insert(type_bits, type_entry);
        }

        let object_bits = MoltObject::from_int(31_002).bits();
        let object_view = ManagedView::Object(Box::new(BridgeHeader {
            py_obj: UnsafeCell::new(PyObject {
                ob_refcnt: 1,
                ob_type: std::ptr::null_mut(),
            }),
        }));
        let object_ptr = object_view.py_obj();
        let object_addr = object_ptr.addr();
        let object_entry = Box::new(BridgeEntry {
            view: object_view,
            bits: object_bits,
            unicode: None,
            publication: PublicationState::Ready,
            lifecycle: BridgeLifecycle::ViewHoldOnly,
        });
        {
            let (mut address, mut handle) =
                bridge.lock_address_then_handle(object_addr, object_bits);
            address.from_py.insert(object_addr, object_bits);
            address.direct_molt_py.insert(object_addr, object_bits);
            handle.to_py.insert(object_bits, object_entry);
        }

        let mut static_object = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let static_ptr = &raw mut static_object;
        let static_addr = static_ptr.addr();
        let static_bits = 0xA110_0000_0000_0090;
        assert!(unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(static_ptr, static_bits, true)
                .is_ok()
        });

        assert!(!bridge.retire_runtime_type_views(&[object_bits]));
        assert_eq!(
            bridge
                .pyobj_to_handle(object_ptr)
                .map(BridgeIdentity::as_handle),
            Some(object_bits)
        );

        assert!(bridge.retire_runtime_type_views(&[type_bits]));
        assert!(bridge.pyobj_to_handle(type_ptr).is_none());
        assert!(
            !bridge
                .address_shard(type_addr)
                .lock()
                .direct_molt_py
                .contains_key(&type_addr)
        );
        assert!(
            !bridge
                .handle_shard(type_bits)
                .lock()
                .to_py
                .contains_key(&type_bits)
        );
        assert_eq!(
            bridge
                .address_shard(static_addr)
                .lock()
                .from_py
                .get(&static_addr),
            Some(&static_bits)
        );
        assert_eq!(
            bridge
                .handle_shard(static_bits)
                .lock()
                .raw_py
                .get(&static_bits)
                .map(|binding| &binding.address),
            Some(&static_addr)
        );
        assert_eq!(
            bridge.release_pyobj(object_ptr),
            PyObjRelease::ManagedViewRetired
        );
        assert!(unsafe { bridge.unbind_static_pyobj_from_runtime_handle(static_ptr, static_bits) });
    }
}
