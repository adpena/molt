//! Generated heap-kind lifecycle dispatch.
//!
//! The generated kind token is the single dispatch key shared by reference
//! counting and cyclic GC. `visit_payload_owned_values` is the exhaustive
//! runtime-payload authority, including inline NaN-boxed values. Physical C
//! owners come from the bridge's mixed-edge visitor. `visit_owned_values` and
//! `visit_owned_edges` retain their managed-only lifecycle projections.
//! `clear_cycle_edges`
//! publishes an empty/cleared state before releasing the mutable subset used to
//! break cycles; immutable ownership edges remain for terminal deallocation.
//! Collector pins remain physical owners throughout projection/resource and
//! semantic-edge retirement. Only GC reachability discounts those pins; the
//! final pin release uses ordinary lifetime/ABI-view transactions.

use super::heap_kinds_generated::{
    HeapLifecycleHandler, HeapTrackProjection, heap_lifecycle_handler, heap_track_projection,
};
use super::{
    HEADER_FLAG_CONTAINS_REFS, HEADER_FLAG_HAS_ABI_VIEW, header_from_obj_ptr, instance_dict_bits,
    object_class_bits, object_class_edge_is_borrowed, object_type_id,
};
use crate::BuiltinExc;
use crate::{MoltObject, PyToken, obj_from_bits};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

thread_local! {
    /// Reentrant pool: a sink is returned only after all detached references
    /// have been released, so a destructor that recursively deallocates another
    /// object acquires a distinct vector rather than aliasing live storage.
    static TERMINAL_EDGE_SINK_POOL: RefCell<[Option<Vec<u64>>; 4]> =
        const { RefCell::new([None, None, None, None]) };
    static TERMINAL_RESOURCE_SINK_POOL: RefCell<[Option<Vec<DetachedResource>>; 4]> =
        const { RefCell::new([None, None, None, None]) };
}

pub(crate) fn touch_terminal_sink_tls_lifetime() {
    let _ = TERMINAL_EDGE_SINK_POOL.try_with(|_| {});
    let _ = TERMINAL_RESOURCE_SINK_POOL.try_with(|_| {});
}

static TERMINAL_EDGE_SINK_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

fn take_best_buffer<T>(slots: &mut [Option<Vec<T>>; 4], required: usize) -> Vec<T> {
    let selected = slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| slot.as_ref().map(|values| (index, values.capacity())))
        .filter(|(_, cached)| *cached >= required)
        .min_by_key(|(_, cached)| *cached)
        .map(|(index, _)| index)
        .or_else(|| {
            slots
                .iter()
                .enumerate()
                .filter_map(|(index, slot)| slot.as_ref().map(|values| (index, values.capacity())))
                .max_by_key(|(_, cached)| *cached)
                .map(|(index, _)| index)
        });
    selected
        .and_then(|index| slots[index].take())
        .unwrap_or_default()
}

/// The largest pooled buffer, for a sink that grows on demand.
fn take_largest_buffer<T>(slots: &mut [Option<Vec<T>>; 4]) -> Vec<T> {
    slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| slot.as_ref().map(|values| (index, values.capacity())))
        .max_by_key(|(_, cached)| *cached)
        .and_then(|(index, _)| slots[index].take())
        .unwrap_or_default()
}

pub(crate) enum DetachedResource {
    /// A list's items, released in place last to first when the resource is
    /// released, at their detachment position among the scalar edges.
    ListItems {
        items: super::backing::TrackedVecContents<u64>,
        preceding_edges: usize,
    },
    DictEntries {
        entries: super::backing::TrackedVecContents<crate::DictEntry>,
        preceding_edges: usize,
    },
    SetEntries {
        entries: super::backing::TrackedVecContents<crate::SetEntry>,
        preceding_edges: usize,
    },
    /// A tuple's canonical C projection after both bridge identity maps have
    /// published it absent.  Its projection-owned references are retired only
    /// after every runtime-owned tuple edge has been detached.
    RuntimeView(molt_cpython_abi::bridge::RetiredRuntimeView),
    ListProjection(molt_cpython_abi::bridge::RetiredClearedListProjection),
    OwnedCFields(molt_cpython_abi::bridge::RetiredOwnedCFields),
    TypeProjection(molt_cpython_abi::bridge::RetiredTypeCycleProjection),
    #[cfg(molt_net)]
    IoSocket(u64),
    #[cfg(molt_net)]
    Websocket(u64),
    Native(super::native_handle::DetachedNativeHandle),
    Foreign(usize),
    NativeBuffer(crate::object::foreign::RuntimeBufferLease),
    FileHandle {
        identity: usize,
        handle: *mut super::MoltFileHandle,
    },
    Functools(crate::builtins::functools::DetachedFunctoolsResource),
    #[cfg(feature = "stdlib_itertools")]
    Itertools(molt_runtime_itertools::itertools::DetachedItertoolsResource),
    #[cfg(not(feature = "stdlib_itertools"))]
    Itertools(crate::builtins::itertools::DetachedItertoolsResource),
}

impl DetachedResource {
    fn preceding_managed_edges(&self) -> Option<usize> {
        match self {
            Self::ListItems {
                preceding_edges, ..
            }
            | Self::DictEntries {
                preceding_edges, ..
            }
            | Self::SetEntries {
                preceding_edges, ..
            } => Some(*preceding_edges),
            _ => None,
        }
    }
}

pub(crate) struct DetachedEdgeSink {
    edges: Vec<u64>,
    resources: Vec<DetachedResource>,
    recycle: bool,
    /// A terminal sink grows as edges are detached; a reserved sink holds
    /// exactly the capacity its caller counted and aborts past it.
    growable: bool,
}

impl DetachedEdgeSink {
    /// Storage for one terminal deallocation, grown as its edges are
    /// detached. The steady state reuses this thread's largest pooled buffers,
    /// so it allocates nothing once the high-water mark is learned. Allocation
    /// failure is process-fatal, as for every terminal sink: unwinding or
    /// partially clearing a refcount-zero object would violate memory safety.
    /// Growth runs no Python code, so the detach walk needs no census first.
    pub(crate) fn terminal() -> Self {
        let (edges, edge_pool_live) = TERMINAL_EDGE_SINK_POOL
            .try_with(|pool| take_largest_buffer(&mut pool.borrow_mut()))
            .map(|edges| (edges, true))
            .unwrap_or_else(|_| (Vec::new(), false));
        let (resources, resource_pool_live) = TERMINAL_RESOURCE_SINK_POOL
            .try_with(|pool| take_largest_buffer(&mut pool.borrow_mut()))
            .map(|resources| (resources, true))
            .unwrap_or_else(|_| (Vec::new(), false));
        Self {
            edges,
            resources,
            recycle: edge_pool_live && resource_pool_live,
            growable: true,
        }
    }

    #[cold]
    #[inline(never)]
    fn grow_edges(&mut self) {
        TERMINAL_EDGE_SINK_ALLOCATIONS.fetch_add(1, AtomicOrdering::Relaxed);
        let additional = self.edges.capacity().max(8);
        if self.edges.try_reserve(additional).is_err() {
            std::alloc::handle_alloc_error(
                std::alloc::Layout::array::<u64>(self.edges.capacity().saturating_add(additional))
                    .unwrap_or_else(|_| std::alloc::Layout::new::<u64>()),
            );
        }
    }

    #[cold]
    #[inline(never)]
    fn grow_resources(&mut self) {
        TERMINAL_EDGE_SINK_ALLOCATIONS.fetch_add(1, AtomicOrdering::Relaxed);
        let additional = self.resources.capacity().max(2);
        if self.resources.try_reserve(additional).is_err() {
            std::alloc::handle_alloc_error(
                std::alloc::Layout::array::<DetachedResource>(
                    self.resources.capacity().saturating_add(additional),
                )
                .unwrap_or_else(|_| std::alloc::Layout::new::<DetachedResource>()),
            );
        }
    }

    pub(crate) fn try_with_capacities(
        edge_capacity: usize,
        resource_capacity: usize,
    ) -> Option<Self> {
        let edges = TERMINAL_EDGE_SINK_POOL
            .try_with(|pool| take_best_buffer(&mut pool.borrow_mut(), edge_capacity))
            .ok()?;
        let resources = TERMINAL_RESOURCE_SINK_POOL
            .try_with(|pool| take_best_buffer(&mut pool.borrow_mut(), resource_capacity))
            .ok()?;
        let mut sink = Self {
            edges,
            resources,
            recycle: true,
            growable: false,
        };
        sink.try_ensure_capacities(edge_capacity, resource_capacity)
            .then_some(sink)
    }

    /// Acquire reusable terminal-deallocation storage. Allocation failure is a
    /// process-fatal memory condition: unwinding or partially clearing a
    /// refcount-zero object would violate memory safety. The steady-state path
    /// performs no allocation once the per-thread high-water mark is learned.
    pub(crate) fn terminal_with_capacities(edge_capacity: usize, resource_capacity: usize) -> Self {
        let (mut edges, edge_pool_live) = TERMINAL_EDGE_SINK_POOL
            .try_with(|pool| take_best_buffer(&mut pool.borrow_mut(), edge_capacity))
            .map(|edges| (edges, true))
            .unwrap_or_else(|_| (Vec::new(), false));
        if edges.capacity() < edge_capacity {
            TERMINAL_EDGE_SINK_ALLOCATIONS.fetch_add(1, AtomicOrdering::Relaxed);
            if edges.try_reserve_exact(edge_capacity).is_err() {
                let layout = std::alloc::Layout::array::<u64>(edge_capacity)
                    .unwrap_or_else(|_| std::alloc::Layout::new::<u64>());
                std::alloc::handle_alloc_error(layout);
            }
        }
        let (mut resources, resource_pool_live) = TERMINAL_RESOURCE_SINK_POOL
            .try_with(|pool| take_best_buffer(&mut pool.borrow_mut(), resource_capacity))
            .map(|resources| (resources, true))
            .unwrap_or_else(|_| (Vec::new(), false));
        if resources.capacity() < resource_capacity {
            TERMINAL_EDGE_SINK_ALLOCATIONS.fetch_add(1, AtomicOrdering::Relaxed);
            if resources.try_reserve_exact(resource_capacity).is_err() {
                let layout = std::alloc::Layout::array::<DetachedResource>(resource_capacity)
                    .unwrap_or_else(|_| std::alloc::Layout::new::<DetachedResource>());
                std::alloc::handle_alloc_error(layout);
            }
        }
        Self {
            edges,
            resources,
            recycle: edge_pool_live && resource_pool_live,
            growable: false,
        }
    }

    #[inline]
    pub(crate) fn detach(&mut self, bits: u64) {
        if self.edges.len() >= self.edges.capacity() {
            if !self.growable {
                // Visit/detach drift after mutation is a memory-safety invariant
                // failure. Never unwind through refcount-zero destruction.
                std::process::abort();
            }
            self.grow_edges();
        }
        self.edges.push(bits);
    }

    #[inline]
    pub(crate) fn detach_if_heap(&mut self, bits: u64) {
        if obj_from_bits(bits).as_ptr().is_some() {
            self.detach(bits);
        }
    }

    pub(crate) fn detach_resource(&mut self, resource: DetachedResource) {
        if self.resources.len() >= self.resources.capacity() {
            if !self.growable {
                std::process::abort();
            }
            self.grow_resources();
        }
        self.resources.push(resource);
    }

    pub(crate) fn try_ensure_capacities(
        &mut self,
        edge_capacity: usize,
        resource_capacity: usize,
    ) -> bool {
        if self.edges.capacity() < edge_capacity
            && self
                .edges
                .try_reserve_exact(edge_capacity.saturating_sub(self.edges.len()))
                .is_err()
        {
            return false;
        }
        if self.resources.capacity() < resource_capacity
            && self
                .resources
                .try_reserve_exact(resource_capacity.saturating_sub(self.resources.len()))
                .is_err()
        {
            return false;
        }
        true
    }

    pub(crate) fn release_all(&mut self, py: &PyToken<'_>) {
        let release_resource = |resource| {
            let release = || match resource {
                DetachedResource::ListItems { items, .. } => {
                    for &bits in items.iter().rev() {
                        crate::dec_ref_bits(py, bits);
                    }
                }
                DetachedResource::DictEntries { entries, .. } => {
                    for row in entries.iter().filter(|row| row.hash.is_some()) {
                        crate::dec_ref_bits(py, row.key);
                        crate::dec_ref_bits(py, row.value);
                    }
                }
                DetachedResource::SetEntries { entries, .. } => {
                    for row in entries.iter().filter(|row| row.hash.is_some()) {
                        crate::dec_ref_bits(py, row.key);
                    }
                }
                DetachedResource::RuntimeView(view) => {
                    crate::object::foreign::retire_native_reference(|| drop(view));
                }
                DetachedResource::ListProjection(projection) => drop(projection),
                DetachedResource::OwnedCFields(fields) => {
                    super::foreign::retire_native_reference(|| drop(fields))
                }
                DetachedResource::TypeProjection(fields) => {
                    crate::object::foreign::retire_native_reference(|| drop(fields));
                }
                #[cfg(molt_net)]
                DetachedResource::IoSocket(bits) => {
                    crate::io_wait_release_detached_resource(py, bits)
                }
                #[cfg(molt_net)]
                DetachedResource::Websocket(bits) => {
                    crate::ws_wait_release_detached_resource(py, bits)
                }
                DetachedResource::Native(handle) => {
                    super::native_handle::native_handle_release(handle)
                }
                DetachedResource::Foreign(pointer) => super::foreign::foreign_release(pointer),
                DetachedResource::NativeBuffer(lease) => drop(lease),
                DetachedResource::FileHandle { identity, handle } => unsafe {
                    if !handle.is_null() {
                        super::flush_file_handle_on_drop(py, &mut *handle);
                        crate::builtins::io::file_handle_close_detached(handle, identity);
                        drop(Box::from_raw(handle));
                    }
                },
                DetachedResource::Functools(resource) => {
                    crate::builtins::functools::functools_release_typed_resources(resource)
                }
                DetachedResource::Itertools(resource) => {
                    #[cfg(feature = "stdlib_itertools")]
                    molt_runtime_itertools::itertools::itertools_release_typed_resources(resource);
                    #[cfg(not(feature = "stdlib_itertools"))]
                    crate::builtins::itertools::itertools_release_typed_resources(resource);
                }
            };
            #[cfg(panic = "unwind")]
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(release)).is_err() {
                eprintln!("molt fatal: detached resource destructor unwound");
                std::process::abort();
            }
            #[cfg(not(panic = "unwind"))]
            release();
        };
        // Physical C/native resources retain their existing precedence. Typed
        // payloads replace scalar edges at the original detachment position;
        // they must not leapfrog common fields or another object's payload.
        for resource in self
            .resources
            .extract_if(.., |resource| resource.preceding_managed_edges().is_none())
        {
            release_resource(resource);
        }
        let mut edges = self.edges.drain(..);
        let mut released = 0usize;
        for resource in self.resources.drain(..) {
            let preceding = resource
                .preceding_managed_edges()
                .expect("only ordered aggregate resources remain");
            while released < preceding {
                let bits = edges
                    .next()
                    .expect("aggregate edge boundary is preflighted");
                crate::dec_ref_bits(py, bits);
                released += 1;
            }
            release_resource(resource);
        }
        for bits in edges {
            crate::dec_ref_bits(py, bits);
        }
    }
}

impl Drop for DetachedEdgeSink {
    fn drop(&mut self) {
        if !self.edges.is_empty() || !self.resources.is_empty() {
            eprintln!(
                "molt fatal: detached lifecycle custody dropped without release (edges={}, resources={})",
                self.edges.len(),
                self.resources.len()
            );
            std::process::abort();
        }
        if self.recycle {
            let mut edges = std::mem::take(&mut self.edges);
            edges.clear();
            let _ = TERMINAL_EDGE_SINK_POOL.try_with(|pool| {
                if let Some(slot) = pool.borrow_mut().iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(edges);
                }
            });
            let mut resources = std::mem::take(&mut self.resources);
            resources.clear();
            let _ = TERMINAL_RESOURCE_SINK_POOL.try_with(|pool| {
                if let Some(slot) = pool.borrow_mut().iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(resources);
                }
            });
        }
    }
}

/// Release the current runtime thread's learned detach-buffer high-water.
/// Runtime shutdown calls this only after every detached owner has drained.
pub(crate) fn reset_detached_sink_pool() {
    TERMINAL_EDGE_SINK_POOL.with(|pool| {
        *pool.borrow_mut() = [None, None, None, None];
    });
    TERMINAL_RESOURCE_SINK_POOL.with(|pool| {
        *pool.borrow_mut() = [None, None, None, None];
    });
}

#[cfg(test)]
pub(crate) fn terminal_edge_sink_allocation_count() -> usize {
    TERMINAL_EDGE_SINK_ALLOCATIONS.load(AtomicOrdering::Relaxed)
}

#[inline(always)]
fn visit_bits<V: FnMut(u64) + ?Sized>(bits: u64, visit: &mut V) {
    visit(bits);
}

#[inline(always)]
fn visit_ptr<V: FnMut(u64) + ?Sized>(ptr: *mut u8, visit: &mut V) {
    if !ptr.is_null() {
        visit(MoltObject::from_ptr(ptr).bits());
    }
}

#[inline]
unsafe fn visit_common_class_edge<V: FnMut(u64) + ?Sized>(ptr: *mut u8, visit: &mut V) {
    if !unsafe { object_class_edge_is_borrowed(ptr) } {
        let class = unsafe { object_class_bits(ptr) };
        if class != 0 {
            visit_bits(class, visit);
        }
    }
}

#[inline]
unsafe fn visit_class_shaped_values<V: FnMut(u64) + ?Sized>(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut V,
) {
    // Fields and the dictionary slot both need a class.
    if unsafe { super::object_has_bare_payload(ptr) }
        || !unsafe { super::object_has_class_shape(ptr) || super::native_instance::has_fields(ptr) }
    {
        return;
    }
    let class_bits = unsafe { object_class_bits(ptr) };
    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
        unsafe {
            super::field_storage::with_instance_fields(py, ptr, class_ptr, |fields| {
                for (_, slot) in fields {
                    if !crate::is_missing_bits(py, *slot) {
                        visit_bits(*slot, visit);
                    }
                }
            });
        }
    }
    let dictionary = unsafe { instance_dict_bits(ptr) };
    if dictionary != 0 {
        visit_bits(dictionary, visit);
    }
}

#[inline]
unsafe fn visit_wrapper_prefix_values<V: FnMut(u64) + ?Sized>(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut V,
) {
    let kind = super::layout::WrapperKind::from_type_id(unsafe { object_type_id(ptr) })
        .expect("wrapper lifecycle handler requires wrapper storage");
    for index in 0..kind.reference_words() {
        let bits = unsafe { super::layout::wrapper_reference_bits(ptr, index) };
        // Raw zero exists only while unpublished allocation is being rolled
        // back. Missing is native initialization state, never a Python referent.
        if bits != 0 && !crate::is_missing_bits(py, bits) {
            visit_bits(bits, visit);
        }
    }
}

#[inline]
unsafe fn visit_native_descriptor_values<V: FnMut(u64) + ?Sized>(ptr: *mut u8, visit: &mut V) {
    for index in 0..super::layout::NATIVE_DESCRIPTOR_REFERENCE_WORDS {
        let bits = unsafe { super::layout::native_descriptor_reference_bits(ptr, index) };
        // Zero is possible only during exclusive unpublished rollback.
        if bits != 0 {
            visit_bits(bits, visit);
        }
    }
}

#[inline]
fn child_requires_tracking(ptr: *mut u8) -> bool {
    match heap_track_projection(unsafe { object_type_id(ptr) }) {
        // Mutable GC-capable children and code with pending metadata publication
        // may acquire a back-edge after insertion.
        // Untracked exact tuples and sealed atomic code cannot acquire new
        // mutable children; an untracked dictionary can.
        Some(HeapTrackProjection::Always | HeapTrackProjection::DictDynamic) => true,
        Some(HeapTrackProjection::CodeDynamic) => unsafe {
            super::gc::gc_is_tracked(ptr)
                || !super::layout::code_declaration_is_published(ptr)
                || super::layout::code_published_execution_kind(ptr).is_none()
        },
        Some(HeapTrackProjection::TupleDynamic) => unsafe {
            super::native_instance::has_fields(ptr) || super::gc::gc_is_tracked(ptr)
        },
        Some(HeapTrackProjection::NativeSubtype) => unsafe {
            super::native_instance::has_fields(ptr)
        },
        Some(HeapTrackProjection::ForeignDynamic) => {
            let address = unsafe { super::foreign::foreign_ptr_from_obj(ptr) };
            unsafe { molt_cpython_abi::bridge::molt_foreign_object_is_gc_capable(address) }
        }
        Some(HeapTrackProjection::Never) | None => false,
    }
}

pub(crate) fn value_requires_tracking(bits: u64) -> bool {
    obj_from_bits(bits)
        .as_ptr()
        .is_some_and(child_requires_tracking)
}

/// Code's tuple-valued metadata is immutable and validated as string tuples.
/// A fresh tuple's conservative GC membership does not make an atomic name
/// cyclic. Inspect its fixed children, preserving mutable native-subclass tails.
pub(crate) fn code_metadata_requires_tracking(bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(ptr) == super::TYPE_ID_TUPLE }
        && !unsafe { super::native_instance::has_fields(ptr) }
    {
        return unsafe {
            super::seq_access::with_immutable_tuple_slice(ptr, |items| {
                items.iter().copied().any(value_requires_tracking)
            })
        }
        .expect("validated immutable code metadata tuple");
    }
    child_requires_tracking(ptr)
}

/// Compute the current CPython-style dynamic tracking projection.
///
/// Dynamic containers are scanned at construction or collection, never once per
/// dictionary write. A false result is valid only while all strong children are
/// permanently atomic; mutable untracked children still require tracking.
pub(crate) unsafe fn projected_track_state(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    match heap_track_projection(unsafe { object_type_id(ptr) })
        .expect("unknown heap kind in track projection")
    {
        HeapTrackProjection::Never => false,
        HeapTrackProjection::Always => true,
        HeapTrackProjection::NativeSubtype => unsafe { super::native_instance::has_fields(ptr) },
        HeapTrackProjection::ForeignDynamic => {
            let address = unsafe { super::foreign::foreign_ptr_from_obj(ptr) };
            (unsafe { molt_cpython_abi::bridge::molt_foreign_object_is_gc_capable(address) })
                && super::gc::native_gc_is_enrolled(address)
        }
        HeapTrackProjection::CodeDynamic => {
            let mut tracked = false;
            unsafe {
                visit_owned_values(py, ptr, &mut |child| {
                    tracked |= code_metadata_requires_tracking(child);
                });
            }
            tracked
        }
        HeapTrackProjection::DictDynamic | HeapTrackProjection::TupleDynamic => {
            // Native fields need a class.
            if !unsafe { super::object_has_bare_payload(ptr) }
                && unsafe { super::native_instance::has_fields(ptr) }
            {
                return true;
            }
            let mut tracked = false;
            unsafe {
                visit_owned_edges(py, ptr, &mut |child| {
                    tracked |= child_requires_tracking(child);
                });
            }
            tracked
        }
    }
}

/// Common scalar ownership, independent of the concrete payload transport.
unsafe fn visit_common_payload_owned_values<V: FnMut(u64) + ?Sized>(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut V,
) {
    if unsafe { super::object_has_bare_payload(ptr) } {
        return;
    }
    unsafe { visit_common_class_edge(ptr, visit) };
    if unsafe { super::native_instance::has_fields(ptr) } {
        unsafe { visit_class_shaped_values(py, ptr, visit) };
    }
    // Activation custody is common sidecar ownership, independent of the task's
    // payload shape or generator/coroutine kind. Preserve alias multiplicity.
    for bits in super::aux_header::object_frame_context_bits(ptr) {
        if bits != 0 {
            visit_bits(bits, visit);
        }
    }
    let awaited = super::aux_header::object_frame_awaited_bits(ptr);
    if awaited != 0 {
        visit_bits(awaited, visit);
    }
    // The frame payload an observer attached to a stateful activation.
    let frame_bindings = super::aux_header::object_frame_bindings_bits(ptr);
    if frame_bindings != 0 {
        visit_bits(frame_bindings, visit);
    }
}

/// Side-effect-free enumeration of runtime payload ownership, including inline
/// values. Independently owned physical C fields are visited separately.
///
/// The match is deliberately exhaustive over the generated per-kind token: adding
/// a heap kind cannot silently inherit an empty traversal lane.
///
/// Generic over the visitor, so a hot consumer (the cycle collector) visits
/// each edge with a direct call; a `dyn` visitor shares one instance.
#[inline]
pub(crate) unsafe fn visit_payload_owned_values<V: FnMut(u64) + ?Sized>(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut V,
) {
    unsafe { visit_common_payload_owned_values(py, ptr, visit) };
    let type_id = unsafe { object_type_id(ptr) };
    let handler = heap_lifecycle_handler(type_id).expect("unknown heap kind in traversal");
    unsafe {
        match handler {
            HeapLifecycleHandler::Object | HeapLifecycleHandler::Weakref => {
                visit_class_shaped_values(py, ptr, visit);
                super::object_shape_visit_owned_edges(py, ptr, |bits| visit_bits(bits, visit));
                if handler == HeapLifecycleHandler::Weakref {
                    super::weakref::weakref_object_visit_owned_edges(py, ptr, |bits| {
                        visit_bits(bits, visit)
                    });
                }
            }
            HeapLifecycleHandler::List => {
                visit_list_items(ptr, visit);
                visit_class_shaped_values(py, ptr, visit);
            }
            HeapLifecycleHandler::Tuple => visit_tuple_items(ptr, visit),
            HeapLifecycleHandler::Dict => visit_dict_rows(ptr, visit),
            HeapLifecycleHandler::Set | HeapLifecycleHandler::Frozenset => {
                let rows = crate::set_entries_ptr(ptr);
                if !rows.is_null() {
                    for row in (*rows).iter().filter(|row| row.hash.is_some()) {
                        visit_bits(row.key, visit);
                    }
                }
            }
            HeapLifecycleHandler::Exception => {
                crate::builtins::exceptions::exception_visit_owned_edges(ptr, |bits| {
                    visit_bits(bits, visit)
                });
            }
            HeapLifecycleHandler::WeakContainerState => {
                super::weak_container::weakcontainer_traverse(ptr, &mut |child| {
                    visit_ptr(child, visit)
                });
            }
            HeapLifecycleHandler::Iter => {
                visit_bits(super::layout::iter_target_bits(ptr), visit);
                visit_ptr(super::layout::iter_cached_tuple(ptr), visit);
            }
            HeapLifecycleHandler::DictKeysView
            | HeapLifecycleHandler::DictValuesView
            | HeapLifecycleHandler::DictItemsView => {
                visit_bits(crate::builtins::containers::dict_view_dict_bits(ptr), visit);
            }
            HeapLifecycleHandler::Slice => {
                visit_bits(super::layout::slice_start_bits(ptr), visit);
                visit_bits(super::layout::slice_stop_bits(ptr), visit);
                visit_bits(super::layout::slice_step_bits(ptr), visit);
            }
            HeapLifecycleHandler::Range => {
                visit_bits(super::layout::range_start_bits(ptr), visit);
                visit_bits(super::layout::range_stop_bits(ptr), visit);
                visit_bits(super::layout::range_step_bits(ptr), visit);
            }
            HeapLifecycleHandler::Memoryview => {
                visit_bits(super::memoryview_owner_bits(ptr), visit);
                if super::memoryview_base_bits(ptr) != super::memoryview_owner_bits(ptr) {
                    visit_bits(super::memoryview_base_bits(ptr), visit);
                }
                visit_bits(super::memoryview_format_bits(ptr), visit);
            }
            HeapLifecycleHandler::Function => {
                for field in super::function_metadata::FunctionMetadataField::ALL {
                    if let Some(bits) = field.load(ptr) {
                        visit_bits(bits, visit);
                    }
                }
                visit_bits(instance_dict_bits(ptr), visit);
                visit_bits(super::layout::function_annotations_bits(ptr), visit);
                visit_bits(super::layout::function_annotate_bits(ptr), visit);
                visit_bits(super::layout::function_code_bits(ptr), visit);
                visit_bits(super::layout::function_closure_bits(ptr), visit);
                visit_bits(super::layout::function_globals_bits(ptr), visit);
                visit_bits(super::layout::function_builtins_bits(ptr), visit);
            }
            HeapLifecycleHandler::BoundMethod => {
                visit_bits(super::layout::bound_method_func_bits(ptr), visit);
                visit_bits(super::layout::bound_method_self_bits(ptr), visit);
                visit_bits(super::layout::bound_method_module_bits(ptr), visit);
            }
            HeapLifecycleHandler::Module => {
                visit_class_shaped_values(py, ptr, visit);
                visit_bits(super::layout::module_name_bits(ptr), visit);
                crate::c_api::c_api_module_visit_owned_edge(py, ptr, |bits| {
                    visit_bits(bits, visit)
                });
            }
            HeapLifecycleHandler::Type => {
                for slot in super::class_storage::ClassReferenceSlot::ALL {
                    visit_bits(slot.load(ptr), visit);
                }
            }
            HeapLifecycleHandler::Classmethod
            | HeapLifecycleHandler::Staticmethod
            | HeapLifecycleHandler::Property => {
                visit_class_shaped_values(py, ptr, visit);
                visit_wrapper_prefix_values(py, ptr, visit);
            }
            HeapLifecycleHandler::NativeDescriptor => {
                visit_native_descriptor_values(ptr, visit);
            }
            HeapLifecycleHandler::Super => {
                visit_bits(super::layout::super_type_bits(ptr), visit);
                visit_bits(super::layout::super_obj_bits(ptr), visit);
                visit_bits(super::layout::super_receiver_class_bits(ptr), visit);
            }
            HeapLifecycleHandler::Enumerate => {
                visit_bits(super::layout::enumerate_target_bits(ptr), visit);
                visit_bits(super::layout::enumerate_index_bits(ptr), visit);
                visit_ptr(super::layout::enumerate_cached_inner(ptr), visit);
                visit_ptr(super::layout::enumerate_cached_outer(ptr), visit);
            }
            HeapLifecycleHandler::CallIter => {
                visit_bits(super::layout::call_iter_sentinel_bits(ptr), visit);
                visit_bits(super::layout::call_iter_callable_bits(ptr), visit);
                visit_ptr(super::layout::call_iter_cached_tuple(ptr), visit);
            }
            HeapLifecycleHandler::Reversed => {
                visit_bits(super::layout::reversed_target_bits(ptr), visit);
            }
            HeapLifecycleHandler::Zip => {
                let iters = super::layout::zip_iters_ptr(ptr);
                if !iters.is_null() {
                    for &bits in &*iters {
                        visit_bits(bits, visit);
                    }
                }
                visit_bits(super::layout::zip_strict_bits(ptr), visit);
                visit_bits(super::layout::zip_result_bits(ptr), visit);
            }
            HeapLifecycleHandler::Map => {
                visit_bits(super::layout::map_func_bits(ptr), visit);
                let iters = super::layout::map_iters_ptr(ptr);
                if !iters.is_null() {
                    for &bits in &*iters {
                        visit_bits(bits, visit);
                    }
                }
                visit_ptr(super::layout::map_cached_tuple(ptr), visit);
            }
            HeapLifecycleHandler::Filter => {
                visit_bits(super::layout::filter_func_bits(ptr), visit);
                visit_bits(super::layout::filter_iter_bits(ptr), visit);
            }
            HeapLifecycleHandler::GenericAlias => {
                visit_bits(super::layout::generic_alias_origin_bits(ptr), visit);
                visit_bits(super::layout::generic_alias_args_bits(ptr), visit);
                visit_bits(super::layout::generic_alias_parameters_bits(ptr), visit);
            }
            HeapLifecycleHandler::Union => {
                visit_bits(super::layout::union_type_args_bits(ptr), visit);
                visit_bits(super::layout::union_type_parameters_bits(ptr), visit);
            }
            HeapLifecycleHandler::TracebackPayload => {
                for slot in crate::builtins::frames::TRACEBACK_PAYLOAD_OWNED_SLOTS {
                    visit_bits(
                        *(ptr.add(slot * std::mem::size_of::<u64>()) as *const u64),
                        visit,
                    );
                }
            }
            HeapLifecycleHandler::ContextBitmapNode
            | HeapLifecycleHandler::ContextCollisionNode => {
                crate::builtins::contextvars::trie::visit(ptr, |bits| visit_bits(bits, visit));
            }
            HeapLifecycleHandler::FrameBindings => {
                crate::builtins::frames::frame_bindings_visit(ptr, &mut |bits| visit(bits));
            }
            HeapLifecycleHandler::ContextManager => {
                visit_bits(crate::builtins::context::context_payload_bits(ptr), visit);
            }

            HeapLifecycleHandler::Code => {
                for slot in [0usize, 1, 3, 4, 5, 12, 13, 14, 15, 16, 19, 20, 22, 23, 24] {
                    visit_bits(
                        *(ptr.add(slot * std::mem::size_of::<u64>()) as *const u64),
                        visit,
                    );
                }
            }
            HeapLifecycleHandler::Cell => {
                visit_bits(super::cells::cell_value_bits(ptr), visit);
            }
            // Shape/custom handlers have their own typed projection authorities.
            HeapLifecycleHandler::Generator => {
                // The task projection includes the saved exception state.
                crate::async_rt::scheduler::task_visit_owned_edges(py, ptr, &mut |bits| {
                    visit_bits(bits, visit)
                });
                // The control region's raw words (state, reserved) are never
                // edges; only its edge slots are visited.
                for offset in crate::GEN_EDGE_OFFSETS {
                    visit_bits(*(ptr.add(offset) as *const u64), visit);
                }
                let payload_size = super::object_payload_size(ptr);
                debug_assert_eq!(payload_size % std::mem::size_of::<u64>(), 0);
                for offset in
                    (crate::GEN_CONTROL_SIZE..payload_size).step_by(std::mem::size_of::<u64>())
                {
                    visit_bits(*(ptr.add(offset) as *const u64), visit);
                }
            }
            HeapLifecycleHandler::AsyncGenerator => {
                crate::async_rt::generators::asyncgen_visit_owned_edges(ptr, |bits| {
                    visit_bits(bits, visit)
                });
            }
            HeapLifecycleHandler::FileHandle => {
                let handle = super::file_handle_ptr(ptr);
                if !handle.is_null() {
                    // Extended instances already project this same external
                    // dictionary through the shared native-field authority.
                    if !super::native_instance::has_fields(ptr) && (*handle).dict_bits != 0 {
                        visit_bits((*handle).dict_bits, visit);
                    }
                    visit_bits((*handle).name_bits, visit);
                    visit_bits((*handle).buffer_bits, visit);
                    visit_bits((*handle).mem_bits, visit);
                }
            }
            HeapLifecycleHandler::Callargs => {
                crate::call::bind::callargs_visit_owned(
                    crate::call::bind::callargs_ptr(ptr),
                    |bits| visit_bits(bits, visit),
                );
            }
            HeapLifecycleHandler::Buffer2d => {
                super::buffer2d::visit_owned_values(super::buffer2d_ptr(ptr), |bits| {
                    visit_bits(bits, visit)
                });
            }
            // Object subshapes are visited after the common inline/dict projection.
            // The typed hook owns poll/operator/itertools/functools/types payloads.
            // Weakref is handled with Object above.
            HeapLifecycleHandler::String
            | HeapLifecycleHandler::Bytes
            | HeapLifecycleHandler::Bytearray
            | HeapLifecycleHandler::Bigint
            | HeapLifecycleHandler::Complex
            | HeapLifecycleHandler::NotImplemented
            | HeapLifecycleHandler::Ellipsis
            | HeapLifecycleHandler::ListInt
            | HeapLifecycleHandler::Float
            | HeapLifecycleHandler::ListBool
            | HeapLifecycleHandler::GlobIter => {}
            HeapLifecycleHandler::NativeHandle | HeapLifecycleHandler::Foreign => {}
            HeapLifecycleHandler::ListBuilder => {
                let values = *(ptr as *mut *mut crate::object::molt_vec::MoltVec<u64>);
                if !values.is_null() {
                    for &bits in &*values {
                        visit_bits(bits, visit);
                    }
                }
            }
        }
    }
}

/// A list's items, first to last.
#[inline(always)]
unsafe fn visit_list_items<V: FnMut(u64) + ?Sized>(ptr: *mut u8, visit: &mut V) {
    unsafe {
        // Unpublished rollback may reach traversal before prefix allocation.
        if !super::layout::seq_vec_ptr(ptr).is_null() {
            crate::object::seq_access::with_borrowed(ptr, |items| {
                for &bits in items {
                    visit_bits(bits, visit);
                }
            });
        }
    }
}

/// A tuple's items, first to last.
#[inline(always)]
unsafe fn visit_tuple_items<V: FnMut(u64) + ?Sized>(ptr: *mut u8, visit: &mut V) {
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
            for &bits in items {
                visit_bits(bits, visit);
            }
        });
    }
}

/// A dict's live rows in row order: each key, then its value.
#[inline(always)]
unsafe fn visit_dict_rows<V: FnMut(u64) + ?Sized>(ptr: *mut u8, visit: &mut V) {
    unsafe {
        let rows = crate::dict_entries_ptr(ptr);
        if !rows.is_null() {
            for row in (*rows).iter().filter(|row| row.hash.is_some()) {
                visit_bits(row.key, visit);
                visit_bits(row.value, visit);
            }
        }
    }
}

/// The owned values of an exact list, tuple or dict whose payload is bare (no
/// class edge, no sidecar) and that has no C view: the containers a
/// collection traverses most. Such an object owns only its kind's items, so
/// this is [`visit_owned_gc_edges`] for it without the class, sidecar,
/// physical-edge, module and memoryview projections, through the same
/// per-kind enumeration. Returns `false`, visiting nothing, for every other
/// object; the caller then takes the full traversal.
#[inline(always)]
pub(crate) unsafe fn visit_bare_container_values<V: FnMut(u64)>(
    ptr: *mut u8,
    visit: &mut V,
) -> bool {
    unsafe {
        if (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW)
            || !super::object_has_bare_payload(ptr)
        {
            return false;
        }
        match object_type_id(ptr) {
            super::TYPE_ID_LIST => visit_list_items(ptr, visit),
            super::TYPE_ID_TUPLE => visit_tuple_items(ptr, visit),
            super::TYPE_ID_DICT => visit_dict_rows(ptr, visit),
            _ => return false,
        }
    }
    true
}

/// Enumerate physical C ownership once, retaining both managed and native edges.
pub(crate) unsafe fn visit_physical_owned_edges<V>(ptr: *mut u8, visit: &mut V)
where
    V: FnMut(molt_cpython_abi::NativeGcEdge) + ?Sized,
{
    if unsafe { (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW) } {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .visit_physical_owned_edges_for_gc(MoltObject::from_ptr(ptr).bits(), &mut |edge| {
                visit(edge)
            });
    }
}

/// Callback-free local ownership inventory. Capacity and tracking consumers use
/// this projection because module-state ownership retires through m_clear/free.
#[inline]
unsafe fn visit_local_owned_gc_edges<V>(py: &PyToken<'_>, ptr: *mut u8, visit: &mut V)
where
    V: FnMut(molt_cpython_abi::NativeGcEdge) + ?Sized,
{
    unsafe {
        visit_payload_owned_values(py, ptr, &mut |bits| {
            visit(molt_cpython_abi::NativeGcEdge {
                kind: molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8,
                reserved: [0; 7],
                value: bits,
            })
        });
        visit_physical_owned_edges(ptr, visit);
        if object_type_id(ptr) == super::TYPE_ID_MEMORYVIEW
            && let Some(lease) = (*super::memoryview_ptr(ptr)).native_lease.as_ref()
            && let Some(edge) = lease.gc_edge()
        {
            visit(edge);
        }
    }
}

/// The complete mixed graph adds extension-owned module state to local owners.
/// Collection streams it without allocating a per-node edge snapshot; the public
/// C traversal boundary snapshots and pins it before invoking its visitor.
/// Generic over the visitor: the cycle collector's traversal makes one direct
/// call per edge.
#[inline]
pub(crate) unsafe fn visit_owned_gc_edges<V>(py: &PyToken<'_>, ptr: *mut u8, visit: &mut V) -> i32
where
    V: FnMut(molt_cpython_abi::NativeGcEdge) + ?Sized,
{
    unsafe {
        visit_local_owned_gc_edges(py, ptr, visit);
        if object_type_id(ptr) == super::TYPE_ID_MODULE {
            return crate::c_api::c_api_module_visit_native_edges(py, ptr, &mut |edge| visit(edge));
        }
    }
    0
}

/// Managed-value projection used by lifecycle capacity and edge-equivalence
/// consumers. Native physical owners are released by their detached resources,
/// not represented as fabricated runtime handles in the managed edge sink.
pub(crate) unsafe fn visit_owned_values(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut dyn FnMut(u64),
) {
    unsafe {
        // Opaque C module state retires through its callbacks. Only declared
        // runtime-owned module slots use the managed sink.
        visit_local_owned_gc_edges(py, ptr, &mut |edge| {
            if edge.kind == molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8 {
                visit_bits(edge.value, visit);
            }
        });
    }
}

/// Heap-pointer projection of [`visit_owned_values`] for managed lifecycle
/// counting and dynamic tracking. Mixed GC consumes visit_owned_gc_edges so
/// native and module-state targets retain their physical ownership identity.
#[inline]
pub(crate) unsafe fn visit_owned_edges<V: FnMut(*mut u8) + ?Sized>(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut V,
) {
    unsafe {
        visit_local_owned_gc_edges(py, ptr, &mut |edge| {
            if edge.kind == molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8
                && let Some(child) = obj_from_bits(edge.value).as_ptr()
            {
                visit(child);
            }
        });
    }
}

pub(crate) unsafe fn detached_resource_count(ptr: *mut u8) -> usize {
    let handler = heap_lifecycle_handler(unsafe { object_type_id(ptr) })
        .expect("unknown heap kind in resource count");
    let projection = usize::from(
        unsafe { (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW) }
            && matches!(
                handler,
                HeapLifecycleHandler::List
                    | HeapLifecycleHandler::Exception
                    | HeapLifecycleHandler::Function
                    | HeapLifecycleHandler::Type
            ),
    );
    projection
        + match handler {
            HeapLifecycleHandler::List => {
                usize::from(!unsafe { super::layout::seq_vec_ptr(ptr) }.is_null())
            }
            HeapLifecycleHandler::Dict => {
                usize::from(!unsafe { crate::dict_entries_ptr(ptr) }.is_null())
            }
            HeapLifecycleHandler::Set | HeapLifecycleHandler::Frozenset => {
                usize::from(!unsafe { crate::set_entries_ptr(ptr) }.is_null())
            }
            HeapLifecycleHandler::Memoryview => {
                usize::from(unsafe { (*super::memoryview_ptr(ptr)).native_lease.is_some() })
            }
            HeapLifecycleHandler::NativeHandle
            | HeapLifecycleHandler::Foreign
            | HeapLifecycleHandler::FileHandle => 1,
            HeapLifecycleHandler::Object | HeapLifecycleHandler::Weakref => {
                usize::from(
                    super::object_shape_resource_slot(super::object_shape_id(ptr))
                        != super::ObjectShapeResourceSlot::None,
                ) + usize::from(matches!(
                    super::object_shape_lifecycle_family(super::object_shape_id(ptr)),
                    super::ObjectShapeLifecycleFamily::Functools
                        | super::ObjectShapeLifecycleFamily::Itertools
                ))
            }
            _ => 0,
        }
}

/// Managed edges transported in the scalar sink. Sparse aggregate edges stay
/// in their charged typed contents until the detached resource is released.
pub(crate) unsafe fn detached_managed_edge_count(py: &PyToken<'_>, ptr: *mut u8) -> usize {
    let mut count = 0usize;
    let mut count_value = |bits| {
        if obj_from_bits(bits).as_ptr().is_some() {
            count = count
                .checked_add(1)
                .unwrap_or_else(|| std::process::abort());
        }
    };
    unsafe {
        match object_type_id(ptr) {
            // Aggregate rows and list items remain in their typed backing.
            // Counting scalar capacity must never walk them only to subtract
            // them. A list's class shape still owns scalar fields.
            type_id @ (super::TYPE_ID_LIST
            | super::TYPE_ID_DICT
            | super::TYPE_ID_SET
            | super::TYPE_ID_FROZENSET) => {
                visit_common_payload_owned_values(py, ptr, &mut count_value);
                if type_id == super::TYPE_ID_LIST {
                    visit_class_shaped_values(py, ptr, &mut count_value);
                }
                visit_physical_owned_edges(ptr, &mut |edge| {
                    if edge.kind == molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8 {
                        count_value(edge.value);
                    }
                });
            }
            _ => visit_owned_values(py, ptr, &mut count_value),
        }
    }
    count
}

/// How terminal retirement releases an object's owned edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalRetirement {
    /// No owned edge but possibly a class, which the caller releases after
    /// the free (`object_detach_class_edge`). No sink and no detach walk.
    ClassEdgeOnly,
    /// The payload's owned edges are a fixed inline set: an exact list's or
    /// tuple's items, an exact dict's rows, a plain instance's fields and
    /// dictionary, an exception's field slots, a call-argument builder's
    /// words, a bound method's three references. The
    /// caller releases them in place with
    /// [`release_owned_in_place`], in the kind's own detach order with no
    /// sink, then the class edge as for `ClassEdgeOnly`.
    ReleaseInPlace,
    /// Every other object takes the full detach transaction: a growable
    /// [`DetachedEdgeSink::terminal`], [`detach_terminal_owned_edges`],
    /// `release_all`.
    Detach,
}

/// Classify the terminal retirement of `ptr`. `header_flags` and `aux_kind`
/// are the terminal snapshot.
///
/// The answer is exact, never a heuristic. Only an object with no side owner
/// (C view, weak reference, scheduler retain) and no sidecar (frame context,
/// awaited value, frame bindings) avoids `Detach`. Of those, an exact list,
/// tuple or dict, a plain class instance, an exception without native fields,
/// a call-argument builder and a bound method are `ReleaseInPlace` whatever
/// their words hold; a slice or range of non-heap words or a leaf kind
/// (string, bytes, big integer, complex, float, integer list, bool list) is
/// `ClassEdgeOnly`. A container is classified in O(1): its items are read
/// once, by the release itself. Inlined into its one caller, the terminal
/// body, so classifying a free costs no call.
#[inline(always)]
pub(crate) unsafe fn terminal_retirement(
    ptr: *mut u8,
    header_flags: u32,
    aux_kind: u16,
) -> TerminalRetirement {
    const SIDE_OWNERS: u32 =
        HEADER_FLAG_HAS_ABI_VIEW | super::HEADER_FLAG_HAS_WEAKREF | crate::HEADER_FLAG_SPAWN_RETAIN;
    if header_flags & SIDE_OWNERS != 0 || aux_kind == super::HEADER_AUX_KIND_SIDECAR {
        return TerminalRetirement::Detach;
    }
    unsafe {
        let class_bits = object_class_bits(ptr);
        if class_bits != 0
            && (!object_class_edge_is_borrowed(ptr) || super::native_instance::has_fields(ptr))
        {
            return if is_plain_instance(ptr) {
                // Its fields and dictionary, then its class.
                TerminalRetirement::ReleaseInPlace
            } else if object_type_id(ptr) == super::TYPE_ID_EXCEPTION
                && !super::native_instance::has_fields(ptr)
            {
                // An instance of an exception class: its fixed field slots
                // (the dictionary among them), then its class.
                TerminalRetirement::ReleaseInPlace
            } else {
                TerminalRetirement::Detach
            };
        }
        let no_heap_words = |words: [u64; 3]| {
            if words.into_iter().any(super::refcount_opt::is_heap_ref) {
                TerminalRetirement::Detach
            } else {
                TerminalRetirement::ClassEdgeOnly
            }
        };
        // Here the class word is empty or borrowed without native fields, so
        // no native subtype field owns an edge.
        match heap_lifecycle_handler(object_type_id(ptr)) {
            // A list's class shape holds fields and a dictionary beside its
            // items; only exact storage qualifies. With no class word and no
            // sidecar a list's shape is plain (`object_shape_id`).
            Some(HeapLifecycleHandler::List)
                if class_bits == 0 || !super::object_has_class_shape(ptr) =>
            {
                TerminalRetirement::ReleaseInPlace
            }
            Some(
                HeapLifecycleHandler::Tuple
                | HeapLifecycleHandler::Callargs
                | HeapLifecycleHandler::BoundMethod
                | HeapLifecycleHandler::Dict
                | HeapLifecycleHandler::Exception,
            ) => TerminalRetirement::ReleaseInPlace,
            Some(HeapLifecycleHandler::Slice) => no_heap_words([
                super::layout::slice_start_bits(ptr),
                super::layout::slice_stop_bits(ptr),
                super::layout::slice_step_bits(ptr),
            ]),
            Some(HeapLifecycleHandler::Range) => no_heap_words([
                super::layout::range_start_bits(ptr),
                super::layout::range_stop_bits(ptr),
                super::layout::range_step_bits(ptr),
            ]),
            // A leaf payload holds bytes or machine words, never a reference:
            // `visit_payload_owned_values`, `clear_cycle_edges_with_sink` and
            // `detach_terminal_owned_edges` do nothing for these kinds, and
            // `detached_resource_count` is zero. The drop policy frees the
            // backing store of an integer or bool list.
            Some(
                HeapLifecycleHandler::String
                | HeapLifecycleHandler::Bytes
                | HeapLifecycleHandler::Bigint
                | HeapLifecycleHandler::Complex
                | HeapLifecycleHandler::Float
                | HeapLifecycleHandler::ListInt
                | HeapLifecycleHandler::ListBool,
            ) => TerminalRetirement::ClassEdgeOnly,
            _ => TerminalRetirement::Detach,
        }
    }
}

/// Release the owned edges of an object [`terminal_retirement`] classified
/// `ReleaseInPlace`, each straight to its release, in the order the full
/// detach transaction uses (`clear_cycle_edges_with_sink`, then
/// `detach_terminal_owned_edges`), through the same per-kind authorities:
///
/// - list: items last to first, CPython's `list_dealloc`; the storage reads
///   empty first and its buffer is freed afterwards by the drop policy;
/// - tuple: `seq_access::detach_tuple_edges`, last to first (`tupledealloc`);
/// - dict: the dictionary reads empty first (`detach_dict_entries`), then
///   each row's key and value in row order (`dict_dealloc`);
/// - exception: `exception_detach_owned_edges`, its field slots in their
///   cycle-clear order;
/// - call arguments: `callargs_detach_owned`, keywords then positionals last
///   to first, as CPython unwinds a value stack;
/// - bound method: the module (the cycle-clear edge), then the function and
///   receiver.
///
/// # Safety
/// `ptr` is committed dead (reference count zero, untracked, weak references
/// cleared) and classified `ReleaseInPlace`, so nothing else reads its words.
/// Inlined into its one caller, the terminal body, whose frame already saves
/// the registers the inlined release edge needs.
#[inline(always)]
pub(crate) unsafe fn release_owned_in_place(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        match object_type_id(ptr) {
            super::TYPE_ID_TUPLE => {
                crate::object::seq_access::detach_tuple_edges(ptr, |bits| {
                    crate::dec_ref_bits(py, bits)
                });
            }
            super::TYPE_ID_LIST => {
                let vec = super::layout::seq_vec_ptr(ptr);
                if vec.is_null() {
                    return;
                }
                let len = (*vec).len();
                (*vec).set_len(0);
                let items = (*vec).as_mut_ptr();
                for index in (0..len).rev() {
                    crate::dec_ref_bits(py, *items.add(index));
                }
            }
            super::TYPE_ID_OBJECT => release_plain_instance_in_place(py, ptr),
            super::TYPE_ID_DICT => {
                if let Some(rows) = detach_dict_entries(py, ptr) {
                    for row in rows.iter().filter(|row| row.hash.is_some()) {
                        crate::dec_ref_bits(py, row.key);
                        crate::dec_ref_bits(py, row.value);
                    }
                }
            }
            _ => release_fixed_words_in_place(py, ptr),
        }
    }
}

/// The few-word kinds of [`release_owned_in_place`], out of line so the hot
/// list and tuple loops keep their inlined release edge.
#[inline(never)]
unsafe fn release_fixed_words_in_place(py: &PyToken<'_>, ptr: *mut u8) {
    let release = |bits| crate::dec_ref_bits(py, bits);
    unsafe {
        match object_type_id(ptr) {
            super::TYPE_ID_CALLARGS => crate::call::bind::callargs_detach_owned(
                crate::call::bind::callargs_ptr(ptr),
                release,
            ),
            super::TYPE_ID_BOUND_METHOD => {
                release(super::layout::bound_method_take_module(ptr));
                for edge in super::layout::bound_method_take_edges(ptr) {
                    release(edge);
                }
            }
            super::TYPE_ID_EXCEPTION => {
                crate::builtins::exceptions::exception_release_detached_edges(
                    crate::builtins::exceptions::exception_detach_owned_edges(ptr),
                    release,
                )
            }
            type_id => {
                eprintln!("molt fatal: type id {type_id} classified ReleaseInPlace");
                std::process::abort();
            }
        }
    }
}

/// Whether `ptr` is a plain class instance: a class-shaped `object` payload
/// without native fields, which owns exactly its fields, its instance
/// dictionary and its class (`clear_class_shaped_edges` and
/// `object_detach_class_edge`).
unsafe fn is_plain_instance(ptr: *mut u8) -> bool {
    unsafe {
        object_type_id(ptr) == crate::TYPE_ID_OBJECT
            && super::object_shape_id(ptr) == super::ObjectShapeId::Plain
            && super::object_has_class_shape(ptr)
            && !super::native_instance::has_fields(ptr)
    }
}

/// Release a plain instance's fields, then its instance dictionary, as
/// CPython's `subtype_dealloc` clears slots before the dictionary
/// (`clear_class_shaped_edges` order). Each slot reads missing before any
/// release runs, and no release runs while the class layout is borrowed:
/// the field values wait in a small buffer.
#[inline(never)]
unsafe fn release_plain_instance_in_place(py: &PyToken<'_>, ptr: *mut u8) {
    const INLINE: usize = 16;
    let mut inline = [0u64; INLINE];
    let mut len = 0usize;
    let mut spill = Vec::new();
    unsafe {
        if let Some(class_ptr) = obj_from_bits(object_class_bits(ptr)).as_ptr() {
            let missing = crate::missing_bits(py);
            super::field_storage::with_instance_fields(py, ptr, class_ptr, |fields| {
                for (_, slot) in fields {
                    let old = slot.replace(missing);
                    if crate::is_missing_bits(py, old) || !super::refcount_opt::is_heap_ref(old) {
                        continue;
                    }
                    if len < INLINE {
                        inline[len] = old;
                        len += 1;
                    } else {
                        if spill.try_reserve(1).is_err() {
                            std::alloc::handle_alloc_error(std::alloc::Layout::new::<u64>());
                        }
                        spill.push(old);
                    }
                }
            });
        }
        let dict = super::instance_dict_bits_ptr(ptr);
        let dict_bits = if dict.is_null() { 0 } else { dict.replace(0) };
        for &bits in &inline[..len] {
            crate::dec_ref_bits(py, bits);
        }
        for bits in spill {
            crate::dec_ref_bits(py, bits);
        }
        if super::refcount_opt::is_heap_ref(dict_bits) {
            crate::dec_ref_bits(py, dict_bits);
        }
    }
}

pub(crate) unsafe fn terminal_detach_capacity(py: &PyToken<'_>, ptr: *mut u8) -> (usize, usize) {
    // Includes both sidecar namespace owners, even when they alias.
    let count = unsafe { detached_managed_edge_count(py, ptr) };
    let handler = heap_lifecycle_handler(unsafe { object_type_id(ptr) })
        .expect("unknown heap kind in terminal capacity");
    let extra = match handler {
        HeapLifecycleHandler::Weakref => {
            super::weakref::weakref_object_terminal_extra_edge_count(py, ptr)
        }
        HeapLifecycleHandler::Iter => {
            let target = unsafe { super::layout::iter_target_bits(ptr) };
            let Some(target_ptr) = obj_from_bits(target).as_ptr() else {
                return (count, unsafe { detached_resource_count(ptr) });
            };
            if unsafe { object_type_id(target_ptr) } != super::TYPE_ID_WEAK_CONTAINER_STATE {
                0
            } else {
                super::weak_container::weakcontainer_iter_finish_detach_edge_count(target_ptr)
            }
        }
        _ => 0,
    };
    // A terminal/explicit close can retire a scheduler reference even though
    // that external root must never be subtracted by cycle graph traversal.
    let scheduler_extra = usize::from(unsafe {
        (*crate::header_from_obj_ptr(ptr)).has_flag(crate::HEADER_FLAG_SPAWN_RETAIN)
    });
    let edges = count
        .checked_add(scheduler_extra)
        .and_then(|count| count.checked_add(extra))
        .unwrap_or_else(|| std::process::abort());
    (edges, unsafe { detached_resource_count(ptr) })
}

/// Detach every generator-owned inline and closure-tail edge (the task
/// detachment that precedes this one takes its saved exception state).
/// The caller reserves the sink before mutation and releases it only after this
/// function has published every source empty.
///
/// Detachment is terminal: the state becomes `Cleared`, which ranks above
/// `Completed` as CPython's `FRAME_CLEARED` does, so no finalizer or
/// resumption re-enters the cleared body. The raw control words are never
/// taken as edges.
pub(crate) unsafe fn detach_generator_owned_edges(ptr: *mut u8, sink: &mut DetachedEdgeSink) {
    unsafe {
        crate::async_rt::generators::generator_clear(ptr);
        for offset in crate::GEN_EDGE_OFFSETS {
            sink.detach_if_heap(super::payload_refs::take(ptr, offset));
        }
        let payload_size = super::object_payload_size(ptr);
        debug_assert_eq!(payload_size % std::mem::size_of::<u64>(), 0);
        for offset in (crate::GEN_CONTROL_SIZE..payload_size).step_by(std::mem::size_of::<u64>()) {
            sink.detach_if_heap(super::payload_refs::take(ptr, offset));
        }
    }
}

/// Close a native coroutine that has never entered its compiled body. The
/// activation's code identity survives for introspection; captures, namespaces,
/// delegation and scheduler owners retire through their existing authorities.
/// Caller owns the coroutine and has verified it is neither started nor running.
pub(crate) unsafe fn close_unstarted_coroutine(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    // CPython 3.12/3.13 mark a created coroutine completed without clearing its
    // frame storage. 3.14 clears that storage during close, before returning.
    if !crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
        crate::task_mark_done(py, ptr);
        return true;
    }
    let (edges, resources) = unsafe { terminal_detach_capacity(py, ptr) };
    let Some(mut sink) = DetachedEdgeSink::try_with_capacities(edges, resources) else {
        crate::raise_exception::<()>(
            py,
            BuiltinExc::MemoryError,
            "cannot retire coroutine activation",
        );
        return false;
    };
    crate::task_mark_done(py, ptr);
    unsafe {
        for bits in super::aux_header::object_take_frame_namespaces_bits(ptr) {
            sink.detach_if_heap(bits);
        }
        sink.detach_if_heap(super::aux_header::object_take_frame_awaited_bits(ptr));
        super::object_shape_clear_cycle_edges(py, ptr, &mut sink);
    }
    sink.release_all(py);
    true
}

#[inline]
unsafe fn clear_class_shaped_edges(py: &PyToken<'_>, ptr: *mut u8, sink: &mut DetachedEdgeSink) {
    if !unsafe { super::object_has_class_shape(ptr) || super::native_instance::has_fields(ptr) } {
        return;
    }
    unsafe {
        super::field_storage::debug::instance(py, "instance_clear", ptr);
    }
    let class_bits = unsafe { object_class_bits(ptr) };
    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
        unsafe {
            super::field_storage::with_instance_fields(py, ptr, class_ptr, |fields| {
                for (_, slot) in fields {
                    let old = slot.replace(crate::missing_bits(py));
                    if !crate::is_missing_bits(py, old) {
                        sink.detach_if_heap(old);
                    }
                }
            });
        }
    }
    let dict = unsafe { super::instance_dict_bits_ptr(ptr) };
    if !dict.is_null() {
        sink.detach_if_heap(unsafe { dict.replace(0) });
    }
}

#[inline]
unsafe fn clear_wrapper_prefix_edges(py: &PyToken<'_>, ptr: *mut u8, sink: &mut DetachedEdgeSink) {
    let kind = super::layout::WrapperKind::from_type_id(unsafe { object_type_id(ptr) })
        .expect("wrapper lifecycle handler requires wrapper storage");
    for index in 0..kind.reference_words() {
        // Every published wrapper proved these defaults available during its
        // unpublished construction. Losing the immortal missing singleton here
        // would violate that representation invariant; never reinterpret it as
        // an explicitly stored None.
        let cleared = super::layout::wrapper_empty_reference_bits(py, kind, index)
            .unwrap_or_else(|| std::process::abort());
        let slot = unsafe { ptr.cast::<u64>().add(index) };
        let old = unsafe { slot.replace(cleared) };
        if old != 0 && !crate::is_missing_bits(py, old) {
            sink.detach_if_heap(old);
        }
    }
}

/// Idempotently detach the mutable cycle-breaking edge subset, then release it.
/// Immutable edges and the common class edge stay owned until terminal dealloc.
#[cfg(test)]
pub(crate) unsafe fn clear_cycle_edges(py: &PyToken<'_>, ptr: *mut u8) {
    assert_eq!(unsafe { try_clear_cycle_edges(py, ptr) }, 0);
}

/// Fallible public single-object clear uses the collector's same detach sink.
/// Extension clear precedes namespace mutation; detached owners release only
/// after every local source has published its empty state.
pub(crate) unsafe fn try_clear_cycle_edges(py: &PyToken<'_>, ptr: *mut u8) -> i32 {
    let Some(mut sink) = DetachedEdgeSink::try_with_capacities(0, 0) else {
        crate::raise_exception::<()>(
            py,
            BuiltinExc::MemoryError,
            "cannot reserve builtin GC clear edges",
        );
        return -1;
    };
    let status = unsafe { try_clear_cycle_edges_with_sink(py, ptr, &mut sink) };
    sink.release_all(py);
    status
}

/// Shared public/collector clear boundary. m_clear can reenter and grow local
/// storage, so reserve against the resulting inventory before detaching it.
/// Terminal destruction uses the callback-free detach primitive after m_free.
pub(crate) unsafe fn try_clear_cycle_edges_with_sink(
    py: &PyToken<'_>,
    ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) -> i32 {
    if unsafe { object_type_id(ptr) } == super::TYPE_ID_MODULE {
        let status = crate::c_api::c_api_module_clear_native_edges(py, ptr);
        if status != 0 {
            return status;
        }
    }
    let count = unsafe { detached_managed_edge_count(py, ptr) };
    let resources = unsafe { detached_resource_count(ptr) };
    if !sink.try_ensure_capacities(
        sink.edges
            .len()
            .checked_add(count)
            .unwrap_or_else(|| std::process::abort()),
        sink.resources
            .len()
            .checked_add(resources)
            .unwrap_or_else(|| std::process::abort()),
    ) {
        crate::raise_exception::<()>(
            py,
            BuiltinExc::MemoryError,
            "cannot reserve builtin GC clear edges",
        );
        return -1;
    }
    unsafe { clear_cycle_edges_with_sink(py, ptr, sink) };
    0
}

/// Publish a dictionary empty and return its rows, whose keys and values the
/// caller releases in row order (`dict_dealloc`). Shared by cycle clear and
/// terminal release.
unsafe fn detach_dict_entries(
    py: &PyToken<'_>,
    ptr: *mut u8,
) -> Option<super::backing::TrackedVecContents<crate::DictEntry>> {
    unsafe {
        super::field_storage::debug::dictionary(
            py,
            "dict_clear_edges",
            std::ptr::null_mut(),
            MoltObject::from_ptr(ptr).bits(),
            None,
            None,
        );
        let rows = crate::dict_entries_ptr(ptr);
        let table = crate::dict_table_ptr(ptr);
        let detached = (!rows.is_null()).then(|| super::backing::tracked_vec_take_contents(rows));
        if !table.is_null() {
            (*table).clear();
        }
        crate::dict_storage(ptr).live = 0;
        crate::dict_storage(ptr).usable = 0;
        crate::dict_storage(ptr).kind = crate::DictKeyKind::Unicode;
        (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
        // The dictionary authority's reset commit: the structural epoch
        // and the namespace binding guard state move together (HF-79).
        super::ops::dict_commit_reset(py, ptr);
        detached
    }
}

pub(crate) unsafe fn clear_cycle_edges_with_sink(
    py: &PyToken<'_>,
    ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    #[inline]
    unsafe fn detach_slots<const N: usize>(ptr: *mut u8, slots: [usize; N]) -> [u64; N] {
        let none = MoltObject::none().bits();
        let mut detached = [0; N];
        for (index, slot) in slots.into_iter().enumerate() {
            let target = unsafe { ptr.add(slot * std::mem::size_of::<u64>()) as *mut u64 };
            detached[index] = unsafe { target.replace(none) };
        }
        detached
    }

    #[inline]
    fn detach(sink: &mut DetachedEdgeSink, detached: impl IntoIterator<Item = u64>) {
        for bits in detached {
            sink.detach_if_heap(bits);
        }
    }

    let type_id = unsafe { object_type_id(ptr) };
    let handler = heap_lifecycle_handler(type_id).expect("unknown heap kind in clear");
    unsafe {
        // A frame object observing an activation takes the bindings over, or its
        // payload stops reading the task, before any task owner is detached.
        crate::builtins::frames::activation_detach_frame_bindings(py, ptr, &mut |bits| {
            sink.detach_if_heap(bits)
        });
        // Clear the complete context before payload detachment can release owners.
        // Terminal detach uses this same path; a preceding GC clear is idempotent.
        detach(sink, super::aux_header::object_take_frame_context_bits(ptr));
        sink.detach_if_heap(super::aux_header::object_take_frame_awaited_bits(ptr));
        if super::native_instance::has_fields(ptr) {
            clear_class_shaped_edges(py, ptr, sink);
        }
        if handler == HeapLifecycleHandler::Weakref {
            super::weakref::weakref_object_detach_owned_edges(py, ptr, sink);
        }
        match handler {
            HeapLifecycleHandler::List => {
                let vec_ptr = super::layout::seq_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    let mutation_guard = super::backing::tracked_vec_mutation_lock(vec_ptr);
                    let detached = super::backing::tracked_vec_take_contents(vec_ptr);
                    let header = &*header_from_obj_ptr(ptr);
                    let clear_abi = header.has_flag(HEADER_FLAG_HAS_ABI_VIEW);
                    header.fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
                    super::backing::tracked_vec_bump_mutation_epoch(vec_ptr);
                    drop(mutation_guard);
                    if clear_abi
                        && let Some(projection) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                            .clear_list_view(MoltObject::from_ptr(ptr).bits())
                    {
                        sink.detach_resource(DetachedResource::ListProjection(projection));
                    }
                    sink.detach_resource(DetachedResource::ListItems {
                        items: detached,
                        preceding_edges: sink.edges.len(),
                    });
                }
                // Fields and the dictionary are independent owners, including
                // when native prefix allocation failed before publication.
                clear_class_shaped_edges(py, ptr, sink);
            }
            HeapLifecycleHandler::Dict => {
                if let Some(detached) = detach_dict_entries(py, ptr) {
                    sink.detach_resource(DetachedResource::DictEntries {
                        entries: detached,
                        preceding_edges: sink.edges.len(),
                    });
                }
            }
            HeapLifecycleHandler::Set => {
                let rows = crate::set_entries_ptr(ptr);
                let table = crate::set_table_ptr(ptr);
                let detached =
                    (!rows.is_null()).then(|| super::backing::tracked_vec_take_contents(rows));
                if !table.is_null() {
                    (*table).clear();
                }
                crate::set_storage(ptr).live = 0;
                crate::set_storage(ptr).fill = 0;
                (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
                if !rows.is_null() {
                    super::backing::tracked_vec_bump_mutation_epoch(rows);
                }
                if let Some(detached) = detached {
                    sink.detach_resource(DetachedResource::SetEntries {
                        entries: detached,
                        preceding_edges: sink.edges.len(),
                    });
                }
            }
            HeapLifecycleHandler::Exception => {
                let detached = crate::builtins::exceptions::exception_detach_owned_edges(ptr);
                if (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW)
                    && let Some(projection) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .clear_owned_c_fields(MoltObject::from_ptr(ptr).bits())
                {
                    sink.detach_resource(DetachedResource::OwnedCFields(projection));
                }
                crate::builtins::exceptions::exception_move_detached_edges(detached, sink);
            }
            HeapLifecycleHandler::Object | HeapLifecycleHandler::Weakref => {
                super::object_shape_clear_cycle_edges(py, ptr, sink);
                clear_class_shaped_edges(py, ptr, sink);
            }
            HeapLifecycleHandler::Classmethod
            | HeapLifecycleHandler::Staticmethod
            | HeapLifecycleHandler::Property => {
                clear_wrapper_prefix_edges(py, ptr, sink);
                clear_class_shaped_edges(py, ptr, sink);
            }
            HeapLifecycleHandler::WeakContainerState => {
                super::weak_container::weakcontainer_detach_state(py, ptr, sink);
            }
            HeapLifecycleHandler::Iter => {
                // Live clear can be observed after a source finalizer revives
                // the iterator; preserve its class before releasing that source.
                // A committed-dead prefix iterator needs no class observation
                // and must not scan its otherwise untouched source on drop.
                if !(*super::header_from_obj_ptr(ptr)).has_flag(super::HEADER_FLAG_DEALLOCATING) {
                    super::ops_iter::resolve_deferred_iterator_class(py, ptr);
                }
                let target = super::layout::iter_target_bits(ptr);
                let cached = super::layout::iter_cached_tuple(ptr);
                super::layout::iter_set_target_bits(ptr, MoltObject::none().bits());
                super::layout::iter_set_cached_tuple(ptr, std::ptr::null_mut());
                if let Some(target_ptr) = obj_from_bits(target).as_ptr()
                    && object_type_id(target_ptr) == super::TYPE_ID_WEAK_CONTAINER_STATE
                {
                    let version = super::layout::iter_expected_version(ptr);
                    if version != super::weak_container::WEAK_ITER_VERSION_UNSTARTED
                        && version != super::weak_container::WEAK_ITER_VERSION_FINISHED
                    {
                        super::weak_container::weakcontainer_iter_finish_detach(
                            py, target_ptr, sink,
                        );
                    }
                    super::layout::iter_set_expected_version(
                        ptr,
                        super::weak_container::WEAK_ITER_VERSION_FINISHED,
                    );
                }
                sink.detach_if_heap(target);
                if !cached.is_null() {
                    sink.detach(MoltObject::from_ptr(cached).bits());
                }
            }
            HeapLifecycleHandler::Function => {
                for field in super::function_metadata::FunctionMetadataField::ALL {
                    sink.detach_if_heap(field.take(ptr));
                }
                let dictionary = super::instance_dict_bits_ptr(ptr);
                super::field_storage::debug::dictionary(
                    py,
                    "function_clear",
                    ptr,
                    *dictionary,
                    None,
                    None,
                );
                sink.detach_if_heap(std::mem::replace(&mut *dictionary, 0));
                detach(sink, detach_slots(ptr, [3, 4, 6, 7, 9, 11]));
                if (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW)
                    && let Some(fields) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .clear_owned_c_fields(MoltObject::from_ptr(ptr).bits())
                {
                    sink.detach_resource(DetachedResource::OwnedCFields(fields));
                }
            }
            HeapLifecycleHandler::Cell => {
                let old = super::cells::cell_detach_value(ptr, crate::missing_bits(py));
                sink.detach_if_heap(old);
            }
            // A binding payload releases what its frame object owns in CPython's
            // order for the path that ends it: destruction (`frame_dealloc`) or
            // cycle collection (`frame_tp_clear`). A clear before destruction
            // leaves nothing for the destruction to release.
            HeapLifecycleHandler::FrameBindings => {
                let flags = (*header_from_obj_ptr(ptr)).load_synchronized_flags();
                let entry = if flags & super::HEADER_FLAG_DEALLOCATING != 0 {
                    crate::builtins::frames::FrameRelease::Destroy
                } else {
                    crate::builtins::frames::FrameRelease::Clear
                };
                crate::builtins::frames::frame_bindings_detach(ptr, entry, |bits| {
                    sink.detach_if_heap(bits)
                });
            }
            HeapLifecycleHandler::Module => {
                clear_class_shaped_edges(py, ptr, sink);
                detach(sink, detach_slots(ptr, [0]));
                crate::c_api::c_api_module_detach_handle_slots(py, ptr, |bits| {
                    sink.detach_if_heap(bits)
                });
            }
            HeapLifecycleHandler::Type => {
                detach(
                    sink,
                    super::class_storage::detach_class_references(
                        ptr,
                        super::class_storage::ClassReferenceRelease::Cycle,
                    ),
                );
                super::class_refresh_declared_finalizer_flag(py, ptr);
                // GC clears the semantic MRO/dictionary and their physical C
                // mirrors together, preserving identity for native owners.
                if let Some(fields) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .clear_type_view_cycle_edges(MoltObject::from_ptr(ptr).bits())
                {
                    sink.detach_resource(DetachedResource::TypeProjection(fields));
                }
            }

            HeapLifecycleHandler::Generator => {
                crate::async_rt::scheduler::task_detach_owned_edges(py, ptr, sink);
                detach_generator_owned_edges(ptr, sink);
            }
            HeapLifecycleHandler::AsyncGenerator => {
                crate::async_rt::generators::asyncgen_detach_owned_edges(ptr, sink);
            }
            HeapLifecycleHandler::FileHandle => {
                let handle = super::file_handle_ptr(ptr);
                if !handle.is_null() {
                    // Native subtype fields clear the external dictionary
                    // first. Exact roots retire it here; no tail dictionary is owned.
                    let detached = [
                        std::mem::replace(&mut (*handle).dict_bits, 0),
                        std::mem::replace(&mut (*handle).name_bits, MoltObject::none().bits()),
                        std::mem::replace(&mut (*handle).buffer_bits, MoltObject::none().bits()),
                        std::mem::replace(&mut (*handle).mem_bits, MoltObject::none().bits()),
                    ];
                    detach(sink, detached);
                }
                let handle = (ptr as *mut *mut super::MoltFileHandle).replace(std::ptr::null_mut());
                sink.detach_resource(DetachedResource::FileHandle {
                    identity: ptr as usize,
                    handle,
                });
            }
            HeapLifecycleHandler::Tuple => {
                // Immutable items remain; shared native fields were cleared above.
            }
            HeapLifecycleHandler::BoundMethod => {
                sink.detach_if_heap(super::layout::bound_method_take_module(ptr))
            }
            // Immutable tracked owners intentionally have no tp_clear. Their edges
            // remain stable and a mutable peer breaks every collectable cycle.
            HeapLifecycleHandler::DictKeysView
            | HeapLifecycleHandler::DictValuesView
            | HeapLifecycleHandler::DictItemsView
            | HeapLifecycleHandler::Slice
            | HeapLifecycleHandler::Memoryview
            | HeapLifecycleHandler::Super
            | HeapLifecycleHandler::Frozenset
            | HeapLifecycleHandler::Enumerate
            | HeapLifecycleHandler::CallIter
            | HeapLifecycleHandler::Reversed
            | HeapLifecycleHandler::Zip
            | HeapLifecycleHandler::Map
            | HeapLifecycleHandler::Filter
            | HeapLifecycleHandler::GenericAlias
            | HeapLifecycleHandler::Union
            | HeapLifecycleHandler::TracebackPayload
            | HeapLifecycleHandler::ContextManager
            | HeapLifecycleHandler::NativeDescriptor
            | HeapLifecycleHandler::String
            | HeapLifecycleHandler::Bytes
            | HeapLifecycleHandler::ListBuilder
            | HeapLifecycleHandler::Bytearray
            | HeapLifecycleHandler::Range
            | HeapLifecycleHandler::Buffer2d
            | HeapLifecycleHandler::Bigint
            | HeapLifecycleHandler::Complex
            | HeapLifecycleHandler::Callargs
            | HeapLifecycleHandler::NotImplemented
            | HeapLifecycleHandler::Code
            | HeapLifecycleHandler::Ellipsis
            | HeapLifecycleHandler::ListInt
            | HeapLifecycleHandler::Float
            | HeapLifecycleHandler::ListBool
            | HeapLifecycleHandler::GlobIter
            | HeapLifecycleHandler::ContextBitmapNode
            | HeapLifecycleHandler::ContextCollisionNode => {}
            HeapLifecycleHandler::NativeHandle => sink.detach_resource(DetachedResource::Native(
                super::native_handle::native_handle_detach(ptr),
            )),
            HeapLifecycleHandler::Foreign => sink.detach_resource(DetachedResource::Foreign(
                super::foreign::foreign_detach(ptr),
            )),
        }
    }
}

/// Publish every Python-owned terminal source empty, across both the mutable
/// cycle-breaking subset and immutable/fixed-layout ownership. The caller passes
/// a growable [`DetachedEdgeSink::terminal`] sink, or one sized from
/// `terminal_detach_capacity` before this mutation phase, and may release it
/// only after this function returns.
pub(crate) unsafe fn detach_terminal_owned_edges(
    py: &PyToken<'_>,
    ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    #[inline]
    unsafe fn detach_slots(
        ptr: *mut u8,
        slots: impl IntoIterator<Item = usize>,
        sink: &mut DetachedEdgeSink,
    ) {
        let none = MoltObject::none().bits();
        for slot in slots {
            let target = unsafe { ptr.add(slot * std::mem::size_of::<u64>()) as *mut u64 };
            sink.detach_if_heap(unsafe { target.replace(none) });
        }
    }

    unsafe { clear_cycle_edges_with_sink(py, ptr, sink) };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id == crate::TYPE_ID_GENERATOR
        || (type_id == crate::TYPE_ID_OBJECT
            && super::object_shape_is_task(super::object_shape_id(ptr)))
    {
        crate::builtins::exceptions::task_raised_slot_forget(py, ptr);
    }
    let handler = heap_lifecycle_handler(type_id).expect("unknown heap kind in terminal detach");
    unsafe {
        match handler {
            HeapLifecycleHandler::Module => {
                if let Some(bits) = crate::c_api::c_api_module_detach_on_teardown(py, ptr) {
                    sink.detach_if_heap(bits);
                }
            }
            HeapLifecycleHandler::Type => {
                for bits in super::class_storage::detach_class_references(
                    ptr,
                    super::class_storage::ClassReferenceRelease::Terminal,
                ) {
                    sink.detach_if_heap(bits);
                }
            }
            HeapLifecycleHandler::ContextBitmapNode
            | HeapLifecycleHandler::ContextCollisionNode => {
                crate::builtins::contextvars::trie::detach(ptr, sink);
            }
            HeapLifecycleHandler::Tuple => {
                crate::object::seq_access::detach_tuple_edges(ptr, |bits| {
                    sink.detach_if_heap(bits)
                });
            }
            HeapLifecycleHandler::Frozenset => {
                let rows = crate::set_entries_ptr(ptr);
                let table = crate::set_table_ptr(ptr);
                let detached =
                    (!rows.is_null()).then(|| super::backing::tracked_vec_take_contents(rows));
                if !table.is_null() {
                    (*table).clear();
                }
                crate::set_storage(ptr).live = 0;
                crate::set_storage(ptr).fill = 0;
                (*header_from_obj_ptr(ptr)).fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
                if !rows.is_null() {
                    super::backing::tracked_vec_bump_mutation_epoch(rows);
                }
                if let Some(detached) = detached {
                    sink.detach_resource(DetachedResource::SetEntries {
                        entries: detached,
                        preceding_edges: sink.edges.len(),
                    });
                }
            }
            HeapLifecycleHandler::DictKeysView
            | HeapLifecycleHandler::DictValuesView
            | HeapLifecycleHandler::DictItemsView
            | HeapLifecycleHandler::Reversed => detach_slots(ptr, [0], sink),
            HeapLifecycleHandler::Union => {
                sink.detach_if_heap(super::layout::union_type_take_parameters(ptr));
                detach_slots(ptr, [0], sink);
            }
            HeapLifecycleHandler::BoundMethod => {
                for edge in super::layout::bound_method_take_edges(ptr) {
                    sink.detach_if_heap(edge);
                }
            }
            HeapLifecycleHandler::GenericAlias => {
                sink.detach_if_heap(super::layout::generic_alias_take_parameters(ptr));
                detach_slots(ptr, [0, 1], sink);
            }
            HeapLifecycleHandler::Filter => detach_slots(ptr, [0, 1], sink),
            HeapLifecycleHandler::Super => detach_slots(ptr, [0, 1, 2], sink),
            HeapLifecycleHandler::Slice | HeapLifecycleHandler::Range => {
                detach_slots(ptr, [0, 1, 2], sink)
            }
            HeapLifecycleHandler::Memoryview => {
                let view = super::memoryview_ptr(ptr);
                let (owner, base, native) = super::buffer_exports::detach_memoryview_owner(ptr);
                if let Some(lease) = native {
                    sink.detach_resource(DetachedResource::NativeBuffer(lease));
                }
                sink.detach_if_heap(owner);
                sink.detach_if_heap(base);
                sink.detach_if_heap(std::mem::replace(
                    &mut (*view).format_bits,
                    MoltObject::none().bits(),
                ));
            }
            HeapLifecycleHandler::Enumerate => {
                detach_slots(ptr, [0, 1], sink);
                let inner = super::layout::enumerate_cached_inner(ptr);
                let outer = super::layout::enumerate_cached_outer(ptr);
                super::layout::enumerate_set_cached_inner(ptr, std::ptr::null_mut());
                super::layout::enumerate_set_cached_outer(ptr, std::ptr::null_mut());
                if !inner.is_null() {
                    sink.detach(MoltObject::from_ptr(inner).bits());
                }
                if !outer.is_null() {
                    sink.detach(MoltObject::from_ptr(outer).bits());
                }
            }
            HeapLifecycleHandler::CallIter => {
                detach_slots(ptr, [0, 1], sink);
                let cached = super::layout::call_iter_cached_tuple(ptr);
                super::layout::call_iter_set_cached_tuple(ptr, std::ptr::null_mut());
                if !cached.is_null() {
                    sink.detach(MoltObject::from_ptr(cached).bits());
                }
            }
            HeapLifecycleHandler::Zip => {
                let iters = super::layout::zip_iters_ptr(ptr);
                if !iters.is_null() {
                    for bits in std::mem::take(&mut *iters).into_iter().rev() {
                        sink.detach_if_heap(bits);
                    }
                }
                let strict = super::layout::zip_strict_bits(ptr);
                super::layout::zip_set_strict_bits(ptr, MoltObject::none().bits());
                sink.detach_if_heap(strict);
                let result = super::layout::zip_result_bits(ptr);
                super::layout::zip_set_result_bits(ptr, MoltObject::none().bits());
                sink.detach_if_heap(result);
            }
            HeapLifecycleHandler::Map => {
                sink.detach_if_heap((ptr as *mut u64).replace(MoltObject::none().bits()));
                let iters = super::layout::map_iters_ptr(ptr);
                if !iters.is_null() {
                    for bits in std::mem::take(&mut *iters) {
                        sink.detach_if_heap(bits);
                    }
                }
                let cached = super::layout::map_cached_tuple(ptr);
                super::layout::map_set_cached_tuple(ptr, std::ptr::null_mut());
                if !cached.is_null() {
                    sink.detach(MoltObject::from_ptr(cached).bits());
                }
            }
            HeapLifecycleHandler::TracebackPayload => detach_slots(
                ptr,
                crate::builtins::frames::TRACEBACK_PAYLOAD_OWNED_SLOTS,
                sink,
            ),
            HeapLifecycleHandler::ContextManager => {
                let payload = ptr.add(2 * std::mem::size_of::<*const ()>()) as *mut u64;
                sink.detach_if_heap(payload.replace(MoltObject::none().bits()));
            }
            HeapLifecycleHandler::Code => {
                crate::builtins::types::dataclasses::drop_code_init_plan(ptr);
                detach_slots(
                    ptr,
                    [0, 1, 3, 4, 5, 12, 13, 14, 15, 16, 19, 20, 22, 23, 24],
                    sink,
                );
            }
            HeapLifecycleHandler::NativeDescriptor => detach_slots(ptr, [0, 1, 2, 3, 4, 5], sink),
            HeapLifecycleHandler::ListBuilder => {
                let values = *(ptr as *mut *mut crate::object::molt_vec::MoltVec<u64>);
                if !values.is_null() {
                    for bits in std::mem::take(&mut *values).into_iter().rev() {
                        sink.detach_if_heap(bits);
                    }
                }
            }
            HeapLifecycleHandler::Callargs => {
                let args = crate::call::bind::callargs_ptr(ptr);
                crate::call::bind::callargs_detach_owned(args, |bits| sink.detach_if_heap(bits));
            }
            HeapLifecycleHandler::Buffer2d => {
                super::buffer2d::detach_owned_values(super::buffer2d_ptr(ptr), |bits| {
                    sink.detach_if_heap(bits)
                });
            }
            // Mutable handlers were fully emptied by clear_cycle_edges_with_sink.
            HeapLifecycleHandler::Object
            | HeapLifecycleHandler::Weakref
            | HeapLifecycleHandler::Classmethod
            | HeapLifecycleHandler::Staticmethod
            | HeapLifecycleHandler::Property
            | HeapLifecycleHandler::List
            | HeapLifecycleHandler::Dict
            | HeapLifecycleHandler::Set
            | HeapLifecycleHandler::Exception
            | HeapLifecycleHandler::WeakContainerState
            | HeapLifecycleHandler::Iter
            | HeapLifecycleHandler::Function
            | HeapLifecycleHandler::Cell
            | HeapLifecycleHandler::FrameBindings
            | HeapLifecycleHandler::Generator
            | HeapLifecycleHandler::AsyncGenerator
            | HeapLifecycleHandler::FileHandle
            | HeapLifecycleHandler::String
            | HeapLifecycleHandler::Bytes
            | HeapLifecycleHandler::Bytearray
            | HeapLifecycleHandler::Bigint
            | HeapLifecycleHandler::Complex
            | HeapLifecycleHandler::NotImplemented
            | HeapLifecycleHandler::Ellipsis
            | HeapLifecycleHandler::ListInt
            | HeapLifecycleHandler::Float
            | HeapLifecycleHandler::ListBool
            | HeapLifecycleHandler::NativeHandle
            | HeapLifecycleHandler::GlobIter
            | HeapLifecycleHandler::Foreign => {}
        }

        sink.detach_if_heap(super::object_detach_class_edge(ptr));
    }
}

#[cfg(test)]
mod detach_sink_tests {
    use super::*;

    #[test]
    fn sparse_dictionary_clear_keeps_detached_values_until_release_and_allows_refill() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
                let value_ptr = obj_from_bits(value).as_ptr().unwrap();
                let owners = || (*header_from_obj_ptr(value_ptr)).ref_count_snapshot();
                let baseline = owners();
                let int = |value| MoltObject::from_int(value).bits();
                let dict = crate::alloc_dict_with_pairs(
                    py,
                    &[int(0), value, int(1), value, int(2), value],
                );
                assert!(crate::object::ops::dict_del_in_place(py, dict, int(1)).unwrap());
                assert_eq!(owners(), baseline + 2);
                let (edges, resources) = terminal_detach_capacity(py, dict);
                let mut sink = DetachedEdgeSink::terminal_with_capacities(edges, resources);
                clear_cycle_edges_with_sink(py, dict, &mut sink);
                assert_eq!(crate::dict_len(dict), 0);
                assert_eq!(
                    owners(),
                    baseline + 2,
                    "clear publishes empty before retiring either live value"
                );
                assert!(crate::object::ops::dict_set_in_place(
                    py,
                    dict,
                    int(9),
                    value
                ));
                assert_eq!(owners(), baseline + 3);
                sink.release_all(py);
                assert_eq!(
                    owners(),
                    baseline + 1,
                    "old detached storage is independent from refilled storage"
                );
                assert_eq!(
                    crate::object::ops::dict_get_in_place(py, dict, int(9)).unwrap(),
                    Some(value)
                );
                crate::dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
                assert_eq!(owners(), baseline);
                crate::dec_ref_bits(py, value);
                assert!(!crate::exception_pending(py));
            }
        });
    }

    /// The full detach transaction is the oracle: a kind retires without the
    /// sink only if that transaction would transport no edge and no resource.
    #[test]
    fn leaf_kinds_retire_without_the_sink_and_the_full_detach_agrees() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let big = crate::builtins::numbers::int_bits_from_i64(py, i64::MAX);
                let leaves = [
                    crate::object::builders::alloc_string_nointern(py, b"not an identifier!"),
                    crate::object::builders::alloc_string_nointern(
                        py,
                        "caf\u{e9} \u{1f600}".as_bytes(),
                    ),
                    crate::alloc_bytes(py, b"\x00\xffbytes"),
                    obj_from_bits(big).as_ptr().expect("heap integer"),
                    crate::object::builders::alloc_list_int_from_raw_slice(py, &[1, -2, 3])
                        .expect("integer list"),
                    crate::object::builders::alloc_list_bool_from_raw_slice(py, &[1, 0])
                        .expect("bool list"),
                ];
                for ptr in leaves {
                    assert!(!ptr.is_null());
                    let flags = (*header_from_obj_ptr(ptr)).load_synchronized_flags();
                    let aux = super::super::object_aux_snapshot(ptr).kind;
                    assert_eq!(
                        terminal_retirement(ptr, flags, aux),
                        TerminalRetirement::ClassEdgeOnly,
                        "type id {}",
                        object_type_id(ptr)
                    );
                    assert_eq!(terminal_detach_capacity(py, ptr), (0, 0));
                    crate::dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
                }
                // A list of references is not a leaf.
                let list = crate::alloc_list(py, &[]);
                let flags = (*header_from_obj_ptr(list)).load_synchronized_flags();
                let aux = super::super::object_aux_snapshot(list).kind;
                assert_eq!(
                    terminal_retirement(list, flags, aux),
                    TerminalRetirement::ReleaseInPlace
                );
                crate::dec_ref_bits(py, MoltObject::from_ptr(list).bits());
                assert!(!crate::exception_pending(py));
            }
        });
    }

    #[test]
    fn terminal_sink_reuses_learned_high_water_without_allocation() {
        let first = DetachedEdgeSink::terminal_with_capacities(64, 0);
        drop(first);
        let learned = terminal_edge_sink_allocation_count();

        let second = DetachedEdgeSink::terminal_with_capacities(64, 0);
        drop(second);
        assert_eq!(terminal_edge_sink_allocation_count(), learned);
    }

    #[test]
    fn nested_terminal_sinks_never_alias_live_storage() {
        let mut outer = DetachedEdgeSink::terminal_with_capacities(1, 0);
        outer.detach(MoltObject::from_ptr(std::ptr::dangling_mut::<u8>()).bits());
        let inner = DetachedEdgeSink::terminal_with_capacities(1, 0);
        assert!(inner.edges.is_empty());
        assert_eq!(outer.edges.len(), 1);
        // This test uses a deliberately dangling non-owning marker only to
        // prove storage separation; remove it before the sink's fail-closed
        // ownership guard runs.
        outer.edges.clear();
        drop(inner);
        drop(outer);
    }
}
