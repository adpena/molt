//! Generated heap-kind lifecycle dispatch.
//!
//! The generated kind token is the single dispatch key shared by reference
//! counting and cyclic GC.  `visit_owned_edges` is side-effect free and lists
//! every strong Python edge released at terminal deallocation.  `clear_cycle_edges`
//! publishes an empty/cleared state before releasing the mutable subset used to
//! break cycles; immutable ownership edges remain for terminal deallocation.

use super::heap_kinds_generated::{
    HeapLifecycleHandler, HeapTrackProjection, heap_lifecycle_handler, heap_track_projection,
};
use super::{
    HEADER_FLAG_CONTAINS_REFS, HEADER_FLAG_HAS_ABI_VIEW, header_from_obj_ptr, instance_dict_bits,
    object_class_bits, object_class_edge_is_borrowed, object_type_id,
};
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

pub(crate) enum DetachedResource {
    /// A tuple's canonical C projection after both bridge identity maps have
    /// published it absent.  Its projection-owned references are retired only
    /// after every runtime-owned tuple edge has been detached.
    RuntimeView(molt_cpython_abi::bridge::RetiredRuntimeView),
    ListProjection(molt_cpython_abi::bridge::RetiredClearedListProjection),
    ExceptionProjection(molt_cpython_abi::bridge::RetiredExceptionProjection),
    IoSocket(u64),
    Websocket(u64),
    Native(super::native_handle::DetachedNativeHandle),
    Foreign(usize),
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

pub(crate) struct DetachedEdgeSink {
    edges: Vec<u64>,
    resources: Vec<DetachedResource>,
    recycle: bool,
}

impl DetachedEdgeSink {
    pub(crate) fn try_with_capacities(
        edge_capacity: usize,
        resource_capacity: usize,
    ) -> Option<Self> {
        let edges = TERMINAL_EDGE_SINK_POOL
            .with(|pool| take_best_buffer(&mut pool.borrow_mut(), edge_capacity));
        let resources = TERMINAL_RESOURCE_SINK_POOL
            .with(|pool| take_best_buffer(&mut pool.borrow_mut(), resource_capacity));
        let mut sink = Self {
            edges,
            resources,
            recycle: true,
        };
        sink.try_ensure_capacities(edge_capacity, resource_capacity)
            .then_some(sink)
    }

    /// Acquire reusable terminal-deallocation storage. Allocation failure is a
    /// process-fatal memory condition: unwinding or partially clearing a
    /// refcount-zero object would violate memory safety. The steady-state path
    /// performs no allocation once the per-thread high-water mark is learned.
    pub(crate) fn terminal_with_capacities(edge_capacity: usize, resource_capacity: usize) -> Self {
        let mut edges = TERMINAL_EDGE_SINK_POOL
            .with(|pool| take_best_buffer(&mut pool.borrow_mut(), edge_capacity));
        if edges.capacity() < edge_capacity {
            TERMINAL_EDGE_SINK_ALLOCATIONS.fetch_add(1, AtomicOrdering::Relaxed);
            if edges.try_reserve_exact(edge_capacity).is_err() {
                let layout = std::alloc::Layout::array::<u64>(edge_capacity)
                    .unwrap_or_else(|_| std::alloc::Layout::new::<u64>());
                std::alloc::handle_alloc_error(layout);
            }
        }
        let mut resources = TERMINAL_RESOURCE_SINK_POOL
            .with(|pool| take_best_buffer(&mut pool.borrow_mut(), resource_capacity));
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
            recycle: true,
        }
    }

    #[inline]
    pub(crate) fn detach(&mut self, bits: u64) {
        if self.edges.len() >= self.edges.capacity() {
            // Visit/detach drift after mutation is a memory-safety invariant
            // failure. Never unwind through refcount-zero destruction.
            std::process::abort();
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
            std::process::abort();
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
        for resource in self.resources.drain(..) {
            let release = || match resource {
                DetachedResource::RuntimeView(view) => drop(view),
                DetachedResource::ListProjection(projection) => drop(projection),
                DetachedResource::ExceptionProjection(projection) => drop(projection),
                DetachedResource::IoSocket(bits) => {
                    crate::io_wait_release_detached_resource(py, bits)
                }
                DetachedResource::Websocket(bits) => {
                    crate::ws_wait_release_detached_resource(py, bits)
                }
                DetachedResource::Native(handle) => {
                    super::native_handle::native_handle_release(handle)
                }
                DetachedResource::Foreign(pointer) => super::foreign::foreign_release(pointer),
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
        }
        for bits in self.edges.drain(..) {
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
            TERMINAL_EDGE_SINK_POOL.with(|pool| {
                if let Some(slot) = pool.borrow_mut().iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(edges);
                }
            });
            let mut resources = std::mem::take(&mut self.resources);
            resources.clear();
            TERMINAL_RESOURCE_SINK_POOL.with(|pool| {
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
fn visit_bits(bits: u64, visit: &mut dyn FnMut(*mut u8)) {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        visit(ptr);
    }
}

#[inline(always)]
fn visit_ptr(ptr: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    if !ptr.is_null() {
        visit(ptr);
    }
}

#[inline]
unsafe fn visit_common_class_edge(ptr: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    if !unsafe { object_class_edge_is_borrowed(ptr) } {
        visit_bits(unsafe { object_class_bits(ptr) }, visit);
    }
}

#[inline]
fn child_requires_tracking(ptr: *mut u8) -> bool {
    match heap_track_projection(unsafe { object_type_id(ptr) }) {
        Some(HeapTrackProjection::Always) => true,
        Some(HeapTrackProjection::DictDynamic | HeapTrackProjection::TupleDynamic) => unsafe {
            super::gc::gc_is_tracked(ptr)
        },
        Some(HeapTrackProjection::Never) | None => false,
    }
}

/// Compute the current CPython-style dynamic tracking projection.
///
/// New dynamic containers begin conservatively tracked. Mutation/finalization
/// sites call this after publishing the new contents; immutable tuples are also
/// reprojected during collection. A false result is valid only while all strong
/// children are atomic/untracked.
pub(crate) unsafe fn projected_track_state(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let type_id = unsafe { object_type_id(ptr) };
    match heap_track_projection(type_id).expect("unknown heap kind in track projection") {
        HeapTrackProjection::Never => false,
        HeapTrackProjection::Always => true,
        HeapTrackProjection::DictDynamic | HeapTrackProjection::TupleDynamic => {
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

/// Side-effect-free, deterministic enumeration of terminally-owned Python edges.
///
/// The match is deliberately exhaustive over the generated per-kind token: adding
/// a heap kind cannot silently inherit an empty traversal lane.
pub(crate) unsafe fn visit_owned_edges(
    py: &PyToken<'_>,
    ptr: *mut u8,
    visit: &mut dyn FnMut(*mut u8),
) {
    unsafe { visit_common_class_edge(ptr, visit) };
    let type_id = unsafe { object_type_id(ptr) };
    let handler = heap_lifecycle_handler(type_id).expect("unknown heap kind in traversal");
    unsafe {
        match handler {
            HeapLifecycleHandler::Object | HeapLifecycleHandler::Weakref => {
                let class_bits = object_class_bits(ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    crate::builtins::attr::for_each_object_inline_field_ptr(
                        py,
                        ptr,
                        class_ptr,
                        &mut |_slot, bits| visit_bits(bits, visit),
                    );
                }
                visit_bits(instance_dict_bits(ptr), visit);
                super::object_shape_visit_owned_edges(py, ptr, |bits| visit_bits(bits, visit));
                if handler == HeapLifecycleHandler::Weakref {
                    super::weakref::weakref_object_visit_owned_edges(py, ptr, |bits| {
                        visit_bits(bits, visit)
                    });
                }
            }
            HeapLifecycleHandler::List => {
                crate::object::seq_access::with_borrowed(ptr, |items| {
                    for &bits in items {
                        visit_bits(bits, visit);
                    }
                });
                if (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
                    for bits in molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .list_view_handles_for_gc(MoltObject::from_ptr(ptr).bits())
                    {
                        visit_bits(bits, visit);
                    }
                }
            }
            HeapLifecycleHandler::Tuple => {
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                    for &bits in items {
                        visit_bits(bits, visit);
                    }
                });
            }
            HeapLifecycleHandler::Dict => {
                let order = crate::builtins::containers::dict_order_ptr(ptr);
                if !order.is_null() {
                    for &bits in &*order {
                        visit_bits(bits, visit);
                    }
                }
            }
            HeapLifecycleHandler::Set | HeapLifecycleHandler::Frozenset => {
                let order = crate::builtins::containers::set_order_ptr(ptr);
                if !order.is_null() {
                    for &bits in &*order {
                        visit_bits(bits, visit);
                    }
                }
            }
            HeapLifecycleHandler::Exception => {
                crate::builtins::exceptions::exception_visit_owned_edges(ptr, |bits| {
                    visit_bits(bits, visit)
                });
                if (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
                    for bits in molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .exception_view_handles_for_gc(MoltObject::from_ptr(ptr).bits())
                    {
                        visit_bits(bits, visit);
                    }
                }
            }
            HeapLifecycleHandler::WeakContainerState => {
                super::weak_container::weakcontainer_traverse(ptr, visit);
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
                visit_bits(super::memoryview_format_bits(ptr), visit);
            }
            HeapLifecycleHandler::Function => {
                visit_bits(super::layout::function_dict_bits(ptr), visit);
                visit_bits(super::layout::function_annotations_bits(ptr), visit);
                visit_bits(super::layout::function_annotate_bits(ptr), visit);
                visit_bits(super::layout::function_code_bits(ptr), visit);
                visit_bits(super::layout::function_closure_bits(ptr), visit);
                visit_bits(super::layout::function_globals_bits(ptr), visit);
            }
            HeapLifecycleHandler::BoundMethod => {
                visit_bits(super::layout::bound_method_func_bits(ptr), visit);
                visit_bits(super::layout::bound_method_self_bits(ptr), visit);
            }
            HeapLifecycleHandler::Module => {
                visit_bits(super::layout::module_dict_bits(ptr), visit);
                visit_bits(super::layout::module_name_bits(ptr), visit);
                crate::c_api::c_api_module_visit_owned_edge(py, ptr, |bits| {
                    visit_bits(bits, visit)
                });
            }
            HeapLifecycleHandler::Type => {
                visit_bits(super::layout::class_name_bits(ptr), visit);
                visit_bits(super::layout::class_bases_bits(ptr), visit);
                visit_bits(super::layout::class_mro_bits(ptr), visit);
                visit_bits(super::layout::class_annotations_bits(ptr), visit);
                visit_bits(super::layout::class_annotate_bits(ptr), visit);
                visit_bits(super::layout::class_qualname_bits(ptr), visit);
                visit_bits(super::layout::class_dict_bits(ptr), visit);
            }
            HeapLifecycleHandler::Classmethod => {
                visit_bits(super::layout::classmethod_func_bits(ptr), visit);
            }
            HeapLifecycleHandler::Staticmethod => {
                visit_bits(super::layout::staticmethod_func_bits(ptr), visit);
            }
            HeapLifecycleHandler::Property => {
                visit_bits(super::layout::property_get_bits(ptr), visit);
                visit_bits(super::layout::property_set_bits(ptr), visit);
                visit_bits(super::layout::property_del_bits(ptr), visit);
            }
            HeapLifecycleHandler::Super => {
                visit_bits(super::layout::super_type_bits(ptr), visit);
                visit_bits(super::layout::super_obj_bits(ptr), visit);
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
            }
            HeapLifecycleHandler::Union => {
                visit_bits(super::layout::union_type_args_bits(ptr), visit);
            }
            HeapLifecycleHandler::TracebackPayload => {
                visit_bits(
                    crate::builtins::frames::traceback_payload_code_bits(ptr),
                    visit,
                );
                visit_bits(
                    crate::builtins::frames::traceback_payload_next_bits(ptr),
                    visit,
                );
            }
            HeapLifecycleHandler::ContextManager => {
                visit_bits(crate::builtins::context::context_payload_bits(ptr), visit);
            }
            HeapLifecycleHandler::Dataclass => {
                let fields = super::dataclass_fields_ptr(ptr);
                if !fields.is_null() {
                    for &bits in &*fields {
                        visit_bits(bits, visit);
                    }
                }
                visit_bits(super::dataclass_dict_bits(ptr), visit);
            }
            HeapLifecycleHandler::Code => {
                for slot in [0usize, 1, 3, 4, 5, 12, 13, 14, 15, 16] {
                    visit_bits(
                        *(ptr.add(slot * std::mem::size_of::<u64>()) as *const u64),
                        visit,
                    );
                }
            }
            // Shape/custom handlers have their own typed projection authorities.
            HeapLifecycleHandler::Generator => {
                crate::builtins::exceptions::generator_exception_stack_visit(ptr, |bits| {
                    visit_bits(bits, visit)
                });
                crate::builtins::context::generator_context_stack_visit(ptr, |bits| {
                    visit_bits(bits, visit)
                });
                visit_bits(*(ptr.add(crate::GEN_SEND_OFFSET) as *const u64), visit);
                visit_bits(*(ptr.add(crate::GEN_THROW_OFFSET) as *const u64), visit);
                visit_bits(*(ptr.add(crate::GEN_CLOSED_OFFSET) as *const u64), visit);
                visit_bits(*(ptr.add(crate::GEN_EXC_DEPTH_OFFSET) as *const u64), visit);
                visit_bits(
                    *(ptr.add(crate::GEN_YIELD_FROM_OFFSET) as *const u64),
                    visit,
                );
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
            // Object subshapes are visited after the common inline/dict projection.
            // The typed hook owns poll/operator/itertools/functools/types payloads.
            // Weakref is handled with Object above.
            HeapLifecycleHandler::String
            | HeapLifecycleHandler::Bytes
            | HeapLifecycleHandler::Bytearray
            | HeapLifecycleHandler::Buffer2d
            | HeapLifecycleHandler::Intarray
            | HeapLifecycleHandler::Bigint
            | HeapLifecycleHandler::Complex
            | HeapLifecycleHandler::NotImplemented
            | HeapLifecycleHandler::Ellipsis
            | HeapLifecycleHandler::ListInt
            | HeapLifecycleHandler::Float
            | HeapLifecycleHandler::ListBool
            | HeapLifecycleHandler::GlobIter => {}
            HeapLifecycleHandler::NativeHandle | HeapLifecycleHandler::Foreign => {}
            HeapLifecycleHandler::ListBuilder
            | HeapLifecycleHandler::DictBuilder
            | HeapLifecycleHandler::SetBuilder => {
                let values = *(ptr as *mut *mut Vec<u64>);
                if !values.is_null() {
                    for &bits in &*values {
                        visit_bits(bits, visit);
                    }
                }
            }
        }
    }
}

pub(crate) unsafe fn detached_resource_count(ptr: *mut u8) -> usize {
    let handler = heap_lifecycle_handler(unsafe { object_type_id(ptr) })
        .expect("unknown heap kind in resource count");
    let projection = usize::from(
        unsafe { (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW) }
            && matches!(
                handler,
                HeapLifecycleHandler::List | HeapLifecycleHandler::Exception
            ),
    );
    projection
        + match handler {
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

pub(crate) unsafe fn terminal_detach_capacity(py: &PyToken<'_>, ptr: *mut u8) -> (usize, usize) {
    let mut count = 0usize;
    unsafe {
        visit_owned_edges(py, ptr, &mut |_| {
            count = count
                .checked_add(1)
                .unwrap_or_else(|| std::process::abort())
        });
    }
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
    let edges = count
        .checked_add(extra)
        .unwrap_or_else(|| std::process::abort());
    (edges, unsafe { detached_resource_count(ptr) })
}

/// Detach every generator-owned inline, closure-tail, and side-registry edge.
/// The caller reserves the sink before mutation and releases it only after this
/// function has published every source empty.
pub(crate) unsafe fn detach_generator_owned_edges(ptr: *mut u8, sink: &mut DetachedEdgeSink) {
    unsafe {
        let none = MoltObject::none().bits();
        for offset in [
            crate::GEN_SEND_OFFSET,
            crate::GEN_THROW_OFFSET,
            crate::GEN_CLOSED_OFFSET,
            crate::GEN_EXC_DEPTH_OFFSET,
            crate::GEN_YIELD_FROM_OFFSET,
        ] {
            sink.detach_if_heap((ptr.add(offset) as *mut u64).replace(none));
        }
        for bits in crate::builtins::exceptions::generator_exception_stack_take(ptr) {
            sink.detach_if_heap(bits);
        }
        for bits in crate::builtins::context::generator_context_stack_take(ptr) {
            sink.detach_if_heap(bits);
        }
        let payload_size = super::object_payload_size(ptr);
        debug_assert_eq!(payload_size % std::mem::size_of::<u64>(), 0);
        for offset in (crate::GEN_CONTROL_SIZE..payload_size).step_by(std::mem::size_of::<u64>()) {
            sink.detach_if_heap((ptr.add(offset) as *mut u64).replace(none));
        }
    }
}

/// Idempotently detach the mutable cycle-breaking edge subset, then release it.
/// Immutable edges and the common class edge stay owned until terminal dealloc.
#[cfg(test)]
pub(crate) unsafe fn clear_cycle_edges(py: &PyToken<'_>, ptr: *mut u8) {
    let mut count = 0usize;
    unsafe {
        visit_owned_edges(py, ptr, &mut |_| {
            count = count
                .checked_add(1)
                .unwrap_or_else(|| std::process::abort())
        })
    };
    let resources = unsafe { detached_resource_count(ptr) };
    let mut sink = DetachedEdgeSink::try_with_capacities(count, resources)
        .expect("single-object clear edge reservation failed");
    unsafe { clear_cycle_edges_with_sink(py, ptr, &mut sink) };
    sink.release_all(py);
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
        if handler == HeapLifecycleHandler::Weakref {
            super::weakref::weakref_object_detach_owned_edges(py, ptr, sink);
        }
        match handler {
            HeapLifecycleHandler::List => {
                let vec_ptr = super::layout::seq_vec_ptr(ptr);
                if vec_ptr.is_null() {
                    return;
                }
                let mutation_guard = super::backing::tracked_vec_mutation_lock(vec_ptr);
                let detached = super::backing::tracked_vec_take_contents(vec_ptr);
                let header = &*header_from_obj_ptr(ptr);
                let clear_abi = header.has_flag(HEADER_FLAG_HAS_ABI_VIEW);
                header.fetch_and_flags(!HEADER_FLAG_CONTAINS_REFS);
                super::backing::tracked_vec_bump_mutation_epoch(vec_ptr);
                drop(mutation_guard);
                if clear_abi {
                    if let Some(projection) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .clear_list_view(MoltObject::from_ptr(ptr).bits())
                    {
                        sink.detach_resource(DetachedResource::ListProjection(projection));
                    }
                }
                for &bits in detached.iter() {
                    sink.detach_if_heap(bits);
                }
                drop(detached);
            }
            HeapLifecycleHandler::Dict => {
                let order = crate::builtins::containers::dict_order_ptr(ptr);
                let table = crate::builtins::containers::dict_table_ptr(ptr);
                let hashes = crate::builtins::containers::dict_hashes_ptr(ptr);
                let detached = if order.is_null() {
                    Vec::new()
                } else {
                    std::mem::take(&mut *order)
                };
                if !table.is_null() {
                    (*table).clear();
                }
                if !hashes.is_null() {
                    (*hashes).clear();
                }
                detach(sink, detached);
            }
            HeapLifecycleHandler::Set => {
                let order = crate::builtins::containers::set_order_ptr(ptr);
                let table = crate::builtins::containers::set_table_ptr(ptr);
                let hashes = crate::builtins::containers::set_hashes_ptr(ptr);
                let detached = if order.is_null() {
                    Vec::new()
                } else {
                    std::mem::take(&mut *order)
                };
                if !table.is_null() {
                    (*table).clear();
                }
                if !hashes.is_null() {
                    (*hashes).clear();
                }
                detach(sink, detached);
            }
            HeapLifecycleHandler::Exception => {
                let detached = crate::builtins::exceptions::exception_detach_owned_edges(ptr);
                if (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
                    if let Some(projection) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .clear_exception_view_fields(MoltObject::from_ptr(ptr).bits())
                    {
                        sink.detach_resource(DetachedResource::ExceptionProjection(projection));
                    }
                }
                crate::builtins::exceptions::exception_move_detached_edges(detached, sink);
            }
            HeapLifecycleHandler::Object | HeapLifecycleHandler::Weakref => {
                super::object_shape_clear_cycle_edges(py, ptr, sink);
                let class_bits = object_class_bits(ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    crate::builtins::attr::for_each_object_inline_field_ptr(
                        py,
                        ptr,
                        class_ptr,
                        &mut |slot, bits| {
                            *slot = 0;
                            sink.detach_if_heap(bits);
                        },
                    );
                }
                let dict = super::instance_dict_bits_ptr(ptr);
                if !dict.is_null() {
                    sink.detach_if_heap(dict.replace(MoltObject::none().bits()));
                }
            }
            HeapLifecycleHandler::WeakContainerState => {
                super::weak_container::weakcontainer_detach_state(py, ptr, sink);
            }
            HeapLifecycleHandler::Iter => {
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
                detach(sink, detach_slots(ptr, [2, 3, 4, 6, 7, 9]));
            }
            HeapLifecycleHandler::Module => {
                detach(sink, detach_slots(ptr, [0, 1]));
                if let Some(bits) = crate::c_api::c_api_module_detach_on_teardown(py, ptr) {
                    sink.detach_if_heap(bits);
                }
            }
            HeapLifecycleHandler::Type => {
                detach(sink, detach_slots(ptr, [0, 2, 3, 5, 6, 7, 1]));
            }
            HeapLifecycleHandler::Dataclass => {
                let fields = super::dataclass_fields_ptr(ptr);
                let detached_fields = if fields.is_null() {
                    Vec::new()
                } else {
                    std::mem::take(&mut *fields)
                };
                let dict = super::dataclass_dict_bits_ptr(ptr);
                let detached_dict = if dict.is_null() {
                    0
                } else {
                    dict.replace(MoltObject::none().bits())
                };
                detach(
                    sink,
                    detached_fields
                        .into_iter()
                        .chain(std::iter::once(detached_dict)),
                );
            }
            HeapLifecycleHandler::Generator => {
                detach_generator_owned_edges(ptr, sink);
            }
            HeapLifecycleHandler::AsyncGenerator => {
                crate::async_rt::generators::asyncgen_detach_owned_edges(ptr, sink);
            }
            HeapLifecycleHandler::FileHandle => {
                let handle = super::file_handle_ptr(ptr);
                if !handle.is_null() {
                    let detached = [
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
            // Immutable tracked owners intentionally have no tp_clear. Their edges
            // remain stable and a mutable peer breaks every collectable cycle.
            HeapLifecycleHandler::Tuple
            | HeapLifecycleHandler::DictKeysView
            | HeapLifecycleHandler::DictValuesView
            | HeapLifecycleHandler::DictItemsView
            | HeapLifecycleHandler::Slice
            | HeapLifecycleHandler::Memoryview
            | HeapLifecycleHandler::BoundMethod
            | HeapLifecycleHandler::Classmethod
            | HeapLifecycleHandler::Staticmethod
            | HeapLifecycleHandler::Property
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
            | HeapLifecycleHandler::String
            | HeapLifecycleHandler::Bytes
            | HeapLifecycleHandler::ListBuilder
            | HeapLifecycleHandler::DictBuilder
            | HeapLifecycleHandler::Bytearray
            | HeapLifecycleHandler::Range
            | HeapLifecycleHandler::Buffer2d
            | HeapLifecycleHandler::Intarray
            | HeapLifecycleHandler::SetBuilder
            | HeapLifecycleHandler::Bigint
            | HeapLifecycleHandler::Complex
            | HeapLifecycleHandler::Callargs
            | HeapLifecycleHandler::NotImplemented
            | HeapLifecycleHandler::Code
            | HeapLifecycleHandler::Ellipsis
            | HeapLifecycleHandler::ListInt
            | HeapLifecycleHandler::Float
            | HeapLifecycleHandler::ListBool
            | HeapLifecycleHandler::GlobIter => {}
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
/// cycle-breaking subset and immutable/fixed-layout ownership. The caller must
/// size `sink` from `visit_owned_edges` before entering this mutation phase and
/// may release it only after this function returns.
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
    let handler = heap_lifecycle_handler(unsafe { object_type_id(ptr) })
        .expect("unknown heap kind in terminal detach");
    unsafe {
        match handler {
            HeapLifecycleHandler::Tuple => {
                crate::object::seq_access::detach_tuple_edges(ptr, |bits| {
                    sink.detach_if_heap(bits)
                });
            }
            HeapLifecycleHandler::Frozenset => {
                let order = crate::builtins::containers::set_order_ptr(ptr);
                if !order.is_null() {
                    let detached = super::backing::tracked_vec_take_contents(order);
                    for &bits in detached.iter() {
                        sink.detach_if_heap(bits);
                    }
                    drop(detached);
                }
                let table = crate::builtins::containers::set_table_ptr(ptr);
                if !table.is_null() {
                    (*table).clear();
                }
                let hashes = crate::builtins::containers::set_hashes_ptr(ptr);
                if !hashes.is_null() {
                    (*hashes).clear();
                }
            }
            HeapLifecycleHandler::DictKeysView
            | HeapLifecycleHandler::DictValuesView
            | HeapLifecycleHandler::DictItemsView
            | HeapLifecycleHandler::Classmethod
            | HeapLifecycleHandler::Staticmethod
            | HeapLifecycleHandler::Reversed
            | HeapLifecycleHandler::Union => detach_slots(ptr, [0], sink),
            HeapLifecycleHandler::BoundMethod
            | HeapLifecycleHandler::Super
            | HeapLifecycleHandler::GenericAlias
            | HeapLifecycleHandler::Filter => detach_slots(ptr, [0, 1], sink),
            HeapLifecycleHandler::Slice | HeapLifecycleHandler::Range => {
                detach_slots(ptr, [0, 1, 2], sink)
            }
            HeapLifecycleHandler::Property => detach_slots(ptr, [0, 1, 2], sink),
            HeapLifecycleHandler::Memoryview => {
                let view = super::memoryview_ptr(ptr);
                sink.detach_if_heap(std::mem::replace(
                    &mut (*view).owner_bits,
                    MoltObject::none().bits(),
                ));
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
                    for bits in std::mem::take(&mut *iters) {
                        sink.detach_if_heap(bits);
                    }
                }
                let strict = super::layout::zip_strict_bits(ptr);
                super::layout::zip_set_strict_bits(ptr, MoltObject::none().bits());
                sink.detach_if_heap(strict);
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
            HeapLifecycleHandler::TracebackPayload => detach_slots(ptr, [0, 4], sink),
            HeapLifecycleHandler::ContextManager => {
                let payload = ptr.add(2 * std::mem::size_of::<*const ()>()) as *mut u64;
                sink.detach_if_heap(payload.replace(MoltObject::none().bits()));
            }
            HeapLifecycleHandler::Code => {
                detach_slots(ptr, [0, 1, 3, 4, 5, 12, 13, 14, 15, 16], sink)
            }
            HeapLifecycleHandler::ListBuilder
            | HeapLifecycleHandler::DictBuilder
            | HeapLifecycleHandler::SetBuilder => {
                let values = *(ptr as *mut *mut Vec<u64>);
                if !values.is_null() {
                    for bits in std::mem::take(&mut *values) {
                        sink.detach_if_heap(bits);
                    }
                }
            }
            HeapLifecycleHandler::Callargs => {
                let args = crate::call::bind::callargs_ptr(ptr);
                crate::call::bind::callargs_detach_owned(py, ptr, args, |bits| {
                    sink.detach_if_heap(bits)
                });
            }
            // Mutable handlers were fully emptied by clear_cycle_edges_with_sink.
            HeapLifecycleHandler::Object
            | HeapLifecycleHandler::Weakref
            | HeapLifecycleHandler::List
            | HeapLifecycleHandler::Dict
            | HeapLifecycleHandler::Set
            | HeapLifecycleHandler::Exception
            | HeapLifecycleHandler::WeakContainerState
            | HeapLifecycleHandler::Iter
            | HeapLifecycleHandler::Function
            | HeapLifecycleHandler::Module
            | HeapLifecycleHandler::Type
            | HeapLifecycleHandler::Dataclass
            | HeapLifecycleHandler::Generator
            | HeapLifecycleHandler::AsyncGenerator
            | HeapLifecycleHandler::FileHandle
            | HeapLifecycleHandler::String
            | HeapLifecycleHandler::Bytes
            | HeapLifecycleHandler::Bytearray
            | HeapLifecycleHandler::Buffer2d
            | HeapLifecycleHandler::Intarray
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
