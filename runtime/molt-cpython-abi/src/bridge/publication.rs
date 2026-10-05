//! Canonical physical-view construction, recursive publication and callable metadata.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;

use super::lifecycle::{
    abort_refcount_invariant, checked_c_ref_increment, initial_managed_view_refs,
};

thread_local! {
    /// Only the building owner may observe its recursive projection skeleton;
    /// the inline stack keeps the common publication path allocation-free.
    static PUBLICATION_BUILD_STACK: RefCell<PublicationBuildStack> = const {
        RefCell::new(PublicationBuildStack::new())
    };

}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PublicationState {
    Building { owner: std::thread::ThreadId },
    Ready,
    Retiring,
}

/// The DFS stack is also the custody of provisional publications. `dependency`
/// is the oldest still-building view observed by a frame or its descendants.
/// A recursive component commits together; independent metadata and exception
/// projections can finish without becoming part of an outer rollback.
struct PublicationFrame {
    bridge: *const ObjectBridge,
    bits: AbiHandle,
    pointer: *mut PyObject,
    dependency: usize,
    complete: bool,
    physical_type_pending: bool,
    failed: bool,
    committed: bool,
    pin_released: bool,
    aborting: bool,
    edges_cleared: bool,
    retired: Option<Box<BridgeEntry>>,
}

struct PublicationBuildStack {
    frames: Vec<PublicationFrame>,
    active: Vec<usize>,
    rolling_back: bool,
}

impl PublicationBuildStack {
    const fn new() -> Self {
        Self {
            frames: Vec::new(),
            active: Vec::new(),
            rolling_back: false,
        }
    }

    fn find(&self, bridge: &ObjectBridge, bits: AbiHandle) -> Option<usize> {
        self.frames.iter().position(|frame| {
            std::ptr::eq(frame.bridge, bridge)
                && frame.bits == bits
                && (!frame.committed || !frame.pin_released)
        })
    }
}

struct PublicationBuildGuard<'a> {
    bridge: &'a ObjectBridge,
    index: usize,
    resolved: bool,
}

impl<'a> PublicationBuildGuard<'a> {
    /// Enter before metadata construction: resolving a base or an exception's
    /// class can itself construct views before this frame has a C allocation.
    fn enter(bridge: &'a ObjectBridge, bits: AbiHandle) -> Option<Self> {
        let admission = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.find(bridge, bits).is_some() {
                return Err(false);
            }
            if stack.frames.try_reserve(1).is_err() || stack.active.try_reserve(1).is_err() {
                return Err(true);
            }
            let index = stack.frames.len();
            stack.frames.push(PublicationFrame {
                bridge,
                bits,
                pointer: std::ptr::null_mut(),
                dependency: index,
                complete: false,
                physical_type_pending: false,
                failed: false,
                committed: false,
                pin_released: false,
                aborting: false,
                edges_cleared: false,
                retired: None,
            });
            stack.active.push(index);
            Ok(index)
        });
        match admission {
            Ok(index) => Some(Self {
                bridge,
                index,
                resolved: false,
            }),
            Err(no_memory) => {
                unsafe {
                    if no_memory {
                        crate::api::errors::PyErr_NoMemory();
                    } else {
                        ensure_result_error(c"recursive ABI metadata has no publication skeleton");
                    }
                }
                None
            }
        }
    }

    fn inserted(&self, pointer: *mut PyObject) {
        PUBLICATION_BUILD_STACK
            .with(|stack| stack.borrow_mut().frames[self.index].pointer = pointer);
    }

    fn finish(mut self) -> bool {
        self.resolve(true)
    }

    fn resolve(&mut self, success: bool) -> bool {
        self.resolved = true;
        let prepare = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let frame = &mut stack.frames[self.index];
            frame.complete = success && !frame.failed;
            frame.complete && frame.dependency == self.index
        });
        // Keep the component's root active through callback-bearing physical
        // readiness. A Type first observed through its MRO cannot be readied
        // until the tuple's last item has been populated. New dependencies
        // discovered by slot lookup join this same transaction.
        let success =
            success && (!prepare || self.bridge.prepare_publication_component(self.index));
        let (success, commit, outermost) = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let success = success && !stack.frames[self.index].failed;
            stack.frames[self.index].complete = success;
            if !success {
                for index in 0..stack.active.len() {
                    let active = stack.active[index];
                    stack.frames[active].failed = true;
                }
            }
            assert_eq!(stack.active.pop(), Some(self.index));
            let dependency = stack.frames[self.index].dependency;
            if let Some(&parent) = stack.active.last() {
                stack.frames[parent].dependency = stack.frames[parent].dependency.min(dependency);
            }
            (
                success,
                success && dependency == self.index,
                self.index == 0 && !stack.rolling_back,
            )
        });
        if commit {
            self.bridge.commit_publication_component(self.index);
        }
        if outermost {
            crate::api::errors::with_preserved_error(|| self.bridge.finish_publication_stack());
        }
        success
    }
}

impl Drop for PublicationBuildGuard<'_> {
    fn drop(&mut self) {
        if !self.resolved {
            self.resolve(false);
        }
    }
}

#[inline]
pub(super) fn is_publication_owner(
    bridge: &ObjectBridge,
    bits: AbiHandle,
    owner: &std::thread::ThreadId,
) -> bool {
    *owner == std::thread::current().id()
        && PUBLICATION_BUILD_STACK.with(|stack| {
            let stack = stack.borrow();
            // Every Building identity is minted by this thread's transaction
            // stack. During its closed edge drain, decrefs need only that owner
            // proof; rescanning the component for each edge is quadratic.
            stack.rolling_back || stack.find(bridge, bits).is_some()
        })
}

/// Record a dependency only when a provisional pointer is observed/retained,
/// not when an edge is released. Return false once rollback has closed it.
pub(super) fn observe_publication(bridge: &ObjectBridge, bits: AbiHandle) -> bool {
    PUBLICATION_BUILD_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(target) = stack.find(bridge, bits) else {
            return true;
        };
        if stack.frames[target].aborting {
            return false;
        }
        if stack.frames[target].committed {
            return true;
        }
        if let Some(&current) = stack.active.last() {
            let dependency = stack.frames[target].dependency;
            stack.frames[current].dependency = stack.frames[current].dependency.min(dependency);
        }
        true
    })
}

/// Why a fully built managed entry could not enter the identity maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ManagedEntryRejection {
    Occupied,
    NoMemory,
    Deallocating,
}

impl ManagedEntryRejection {
    unsafe fn set_error(self) {
        match self {
            Self::Occupied => unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"ABI view runtime identity was already published".as_ptr(),
                )
            },
            Self::NoMemory => unsafe {
                crate::api::errors::PyErr_NoMemory();
            },
            Self::Deallocating => unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RuntimeError).cast::<PyObject>(),
                    c"cannot publish ABI view for deallocating object".as_ptr(),
                )
            },
        }
    }
}

impl ObjectBridge {
    unsafe fn managed_type_creation_doc(bits: AbiHandle) -> Option<*const std::os::raw::c_char> {
        use crate::hooks::{DecodedHandleResult, TypeMetadataField};
        let hooks = crate::hooks::hooks_or_stubs();
        match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::CreationDoc) }.decode() {
            DecodedHandleResult::Missing => Some(std::ptr::null()),
            DecodedHandleResult::Error => None,
            DecodedHandleResult::Ok(doc) => {
                let mut length = 0;
                let bytes = unsafe { (hooks.str_data)(doc, &raw mut length) };
                // CreationDoc owns immutable terminated runtime string storage
                // until class terminal release. The already-held class anchor
                // owns the lifetime; this projection adds no string/C owner.
                unsafe { (hooks.dec_ref)(doc) };
                if bytes.is_null() {
                    unsafe { ensure_result_error(c"invalid managed type creation documentation") };
                    None
                } else {
                    Some(bytes.cast())
                }
            }
        }
    }

    pub(super) unsafe fn managed_type_metadata(
        bits: AbiHandle,
    ) -> Option<(std::ffi::CString, Option<u64>, std::os::raw::c_ulong)> {
        use crate::hooks::{DecodedHandleResult, TypeMetadataField};
        let hooks = crate::hooks::hooks_or_stubs();
        let name_bits =
            match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::Name) }.decode() {
                DecodedHandleResult::Ok(name) => name,
                DecodedHandleResult::Missing | DecodedHandleResult::Error => {
                    unsafe { ensure_result_error(c"managed type name unavailable") };
                    return None;
                }
            };
        let mut length = 0usize;
        let bytes = unsafe { (hooks.str_data)(name_bits, &raw mut length) };
        let name = if bytes.is_null() {
            None
        } else {
            // Heap tp_name is the current __name__, not module/qualname. Keep
            // WTF-8 intact; constructing this projection must not run Python.
            std::ffi::CString::new(unsafe { std::slice::from_raw_parts(bytes, length) }).ok()
        };
        unsafe { (hooks.dec_ref)(name_bits) };
        let Some(name) = name else {
            unsafe { ensure_result_error(c"invalid managed type name") };
            return None;
        };
        let flags_bits =
            match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::SemanticFlags) }.decode()
            {
                DecodedHandleResult::Ok(flags) => flags,
                _ => {
                    unsafe { ensure_result_error(c"managed type semantic flags unavailable") };
                    return None;
                }
            };
        let flags = MoltObject::from_bits(flags_bits).as_int();
        unsafe { (hooks.dec_ref)(flags_bits) };
        let Some(flags) = flags.filter(|value| *value >= 0) else {
            unsafe { ensure_result_error(c"invalid managed type semantic flags") };
            return None;
        };
        let base_bits =
            match unsafe { (hooks.type_metadata)(bits, TypeMetadataField::Base) }.decode() {
                DecodedHandleResult::Ok(base) => Some(base),
                DecodedHandleResult::Missing => None,
                DecodedHandleResult::Error => {
                    unsafe { ensure_result_error(c"managed type base unavailable") };
                    return None;
                }
            };
        Some((name, base_bits, flags as std::os::raw::c_ulong))
    }
}

// Fallible physical-view construction prior to publication.
impl ObjectBridge {
    /// Resolve the runtime's borrowed class identity through the one bridge
    /// projection authority. Static builtin bindings and managed Type views
    /// both come from `handle_to_borrowed_pyobj`; neither changes the physical
    /// storage type of the value whose class is requested.
    pub(super) unsafe fn runtime_class_view(&self, bits: u64) -> *mut PyTypeObject {
        let result = unsafe { (crate::hooks::hooks_or_stubs().runtime_class_borrowed)(bits) };
        let class_bits = match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(class_bits) if class_bits != 0 => class_bits,
            _ => {
                unsafe { ensure_result_error(c"managed runtime class identity unavailable") };
                return std::ptr::null_mut();
            }
        };
        let view = unsafe { self.handle_to_borrowed_pyobj(class_bits) };
        if view.is_null() {
            unsafe { ensure_result_error(c"managed runtime class has no ABI Type view") };
        }
        view.cast::<PyTypeObject>()
    }

    pub(super) unsafe fn build_pyobj_entry(
        &self,
        bits: AbiHandle,
        ob_refcnt: isize,
        internal_c_ref: bool,
    ) -> Option<(Box<BridgeEntry>, *mut PyObject)> {
        let tag = Self::classify_handle(bits);
        let ob_type = if tag == MoltTypeTag::Exception {
            let class_view = unsafe { self.runtime_class_view(bits) };
            if class_view.is_null() {
                return None;
            }
            class_view
        } else {
            unsafe { tag_to_type(tag) }
        };
        let view = if tag == MoltTypeTag::Type {
            let creation_doc = unsafe { Self::managed_type_creation_doc(bits) }?;
            let (name, base_bits, semantic_flags) = unsafe { Self::managed_type_metadata(bits) }?;
            let exception_layout = ExceptionLayoutKind::from_u8(unsafe {
                (crate::hooks::hooks_or_stubs().exception_layout_kind)(bits)
            });
            let base = if let Some(base_bits) = base_bits {
                let base_view = unsafe { self.handle_to_borrowed_pyobj(base_bits) };
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(base_bits) };
                if base_view.is_null() {
                    return None;
                }
                base_view.cast::<PyTypeObject>()
            } else {
                &raw mut PyBaseObject_Type
            };
            let inherited_flags = if base.is_null() {
                0
            } else {
                unsafe {
                    (*base).tp_flags
                        & (crate::abi_types::Py_TPFLAGS_LONG_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_LIST_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_TUPLE_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_BYTES_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_UNICODE_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_DICT_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_BASE_EXC_SUBCLASS
                            | crate::abi_types::Py_TPFLAGS_TYPE_SUBCLASS)
                }
            };
            let mut object: PyTypeObject = unsafe { std::mem::zeroed() };
            object.ob_base = crate::abi_types::PyVarObject {
                ob_base: PyObject {
                    ob_refcnt,
                    ob_type: &raw mut PyType_Type,
                },
                ob_size: 0,
            };
            object.tp_name = name.as_ptr();
            object.tp_doc = creation_doc;
            object.tp_basicsize = if let Some(layout) = exception_layout {
                crate::abi_types::exception_layout_basicsize(layout)
            } else if base.is_null() {
                std::mem::size_of::<PyObject>() as crate::abi_types::Py_ssize_t
            } else {
                unsafe { (*base).tp_basicsize }
            };
            object.tp_itemsize = if exception_layout.is_some() || base.is_null() {
                0
            } else {
                unsafe { (*base).tp_itemsize }
            };
            object.tp_flags = crate::abi_types::Py_TPFLAGS_DEFAULT
                | semantic_flags
                | inherited_flags
                | if exception_layout.is_some() {
                    crate::abi_types::Py_TPFLAGS_BASE_EXC_SUBCLASS
                        | crate::abi_types::Py_TPFLAGS_HAVE_GC
                } else {
                    0
                };
            object.tp_base = base;
            ManagedView::Type {
                object: ManagedTypeAllocation::new(object),
                _name: name,
            }
        } else if tag == MoltTypeTag::Slice {
            ManagedView::Slice(Box::new(UnsafeCell::new(crate::abi_types::PySliceObject {
                ob_base: PyObject { ob_refcnt, ob_type },
                start: std::ptr::null_mut(),
                stop: std::ptr::null_mut(),
                step: std::ptr::null_mut(),
            })))
        } else if tag == MoltTypeTag::MemoryView {
            let mut object: crate::abi_types::PyMemoryViewObject = unsafe { std::mem::zeroed() };
            object.ob_base = PyObject { ob_refcnt, ob_type };
            ManagedView::MemoryView {
                object: Box::new(UnsafeCell::new(object)),
                format: c"B".to_owned(),
            }
        } else if tag == MoltTypeTag::BuiltinCallable {
            let mut object: PyCMethodObject = unsafe { std::mem::zeroed() };
            object.func.ob_base = PyObject { ob_refcnt, ob_type };
            object.func.vectorcall = Some(crate::api::object::molt_runtime_vectorcall);
            ManagedView::RuntimeCallable(Box::new(UnsafeCell::new(object)))
        } else if tag == MoltTypeTag::Tuple {
            let len = unsafe { (crate::hooks::hooks_or_stubs().tuple_len)(bits) };
            let allocation = TupleAllocation::new(ob_refcnt, ob_type, len)?;
            ManagedView::Tuple { allocation }
        } else if tag == MoltTypeTag::List {
            let len = unsafe { (crate::hooks::hooks_or_stubs().list_len)(bits) };
            let allocation = ListAllocation::new(ob_refcnt, ob_type, len)?;
            ManagedView::List { allocation }
        } else if tag == MoltTypeTag::Exception {
            let raw_kind = unsafe { (crate::hooks::hooks_or_stubs().exception_layout_kind)(bits) };
            let layout_kind = ExceptionLayoutKind::from_u8(raw_kind)?;
            if ob_type.is_null()
                || unsafe { (*ob_type).tp_basicsize }
                    != crate::abi_types::exception_layout_basicsize(layout_kind)
            {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                        c"managed exception type/layout size mismatch".as_ptr(),
                    )
                };
                return None;
            }
            ManagedView::Exception(ExceptionAllocation::new(layout_kind, ob_refcnt, ob_type))
        } else {
            ManagedView::Object(Box::new(BridgeHeader {
                py_obj: UnsafeCell::new(PyObject { ob_refcnt, ob_type }),
            }))
        };
        let unicode = if tag == MoltTypeTag::Str {
            let mut len = 0;
            let data = unsafe { (crate::hooks::hooks_or_stubs().str_data)(bits, &mut len) };
            if data.is_null() {
                return None;
            }
            let bytes = unsafe { std::slice::from_raw_parts(data, len) };
            let Some(text) = crate::api::strings::PythonStringBytes::from_bytes(bytes) else {
                unsafe { ensure_result_error(c"invalid internal Python string storage") };
                return None;
            };
            let Some(projection) = UnicodeProjection::from_text(text) else {
                unsafe { crate::api::errors::PyErr_NoMemory() };
                return None;
            };
            Some(projection)
        } else {
            None
        };
        let entry = Box::new(BridgeEntry {
            view,
            bits,
            unicode,
            publication: PublicationState::Building {
                owner: std::thread::current().id(),
            },
            lifecycle: if internal_c_ref {
                BridgeLifecycle::RuntimeOwned
            } else {
                BridgeLifecycle::ViewHoldOnly
            },
        });
        let raw_ptr = entry.view.py_obj();
        Some((entry, raw_ptr))
    }

    pub(super) unsafe fn published_pyobj(
        &self,
        bits: AbiHandle,
        increment: bool,
    ) -> Option<*mut PyObject> {
        let index = self.handle_shard_index(bits);
        let mut handle = self.handle_shards[index].lock();
        loop {
            if let Some(entry) = handle.to_py.get(&bits) {
                match &entry.publication {
                    PublicationState::Ready => {
                        let ptr = entry.view.py_obj();
                        if increment {
                            unsafe { Self::increment_pyobj_ref(ptr) };
                        }
                        return Some(ptr);
                    }
                    PublicationState::Building { owner }
                        if is_publication_owner(self, bits, owner) =>
                    {
                        if !observe_publication(self, bits) {
                            return Some(std::ptr::null_mut());
                        }
                        let ptr = entry.view.py_obj();
                        if increment {
                            unsafe { Self::increment_pyobj_ref(ptr) };
                        }
                        return Some(ptr);
                    }
                    PublicationState::Building { .. } | PublicationState::Retiring => {
                        self.publication_ready[index].wait(&mut handle);
                        continue;
                    }
                }
            }
            if let Some(binding) = handle.raw_py.get(&bits) {
                let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(binding.address);
                if increment {
                    unsafe { Self::increment_pyobj_ref(ptr) };
                }
                return Some(ptr);
            }
            return None;
        }
    }

    /// Release a transaction pin/edge through the same bridge that owns it.
    /// Non-managed edges retain the ordinary native/numeric release authority.
    pub(super) unsafe fn decref_publication_reference(&self, pointer: *mut PyObject) {
        if pointer.is_null() {
            return;
        }
        match unsafe { self.managed_decref_pyobj(pointer) } {
            ManagedDecref::Immortal | ManagedDecref::Alive | ManagedDecref::RetiredInline => {}
            ManagedDecref::ReleaseRuntimeHold(bits) => unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            },
            ManagedDecref::NotManaged => unsafe {
                crate::api::refcount::Py_DECREF(pointer);
            },
        }
    }

    fn prepare_publication_component(&self, first: usize) -> bool {
        let mut index = first;
        loop {
            let deferred = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let root = &stack.frames[first];
                (root.failed, root.dependency < first)
            });
            if deferred.0 {
                return false;
            }
            if deferred.1 {
                // A readiness callback discovered an older component. Its
                // root will finish the remaining physical work after all of
                // its metadata is populated; do not commit this suffix alone.
                return true;
            }
            let candidate = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                stack.frames.get(index).map(|frame| {
                    (
                        frame.bridge,
                        frame.bits,
                        frame.pointer,
                        frame.physical_type_pending && !frame.committed,
                    )
                })
            });
            let Some((bridge, bits, pointer, pending)) = candidate else {
                return true;
            };
            if pending {
                let bridge = unsafe { &*bridge };
                // The transaction pin owns this exact allocation throughout
                // callbacks. No address/handle/stack lock crosses readiness.
                if bridge.managed_handle_for_pyobj(pointer) != Some(bits)
                    || unsafe { crate::api::typeobj::ready_type(bridge, pointer.cast()) } < 0
                    || bridge.managed_handle_for_pyobj(pointer) != Some(bits)
                {
                    return false;
                }
            }
            index += 1;
        }
    }

    /// Public readiness may not consume a Type's provisional graph while any
    /// frame in its recursive component is still filling metadata.
    pub(crate) fn managed_type_population_complete(&self, pointer: *mut PyTypeObject) -> bool {
        let Some(bits) = self.managed_handle_for_pyobj(pointer.cast()) else {
            return true;
        };
        PUBLICATION_BUILD_STACK.with(|stack| {
            let stack = stack.borrow();
            let Some(index) = stack.find(self, bits) else {
                return true;
            };
            let dependency = stack.frames[index].dependency;
            stack.frames[dependency..]
                .iter()
                .all(|frame| frame.committed || (frame.complete && !frame.failed))
        })
    }

    pub(super) fn commit_publication_component(&self, first: usize) {
        let end = PUBLICATION_BUILD_STACK.with(|stack| stack.borrow().frames.len());
        // Commit the complete component irreversibly before releasing a pin can
        // run a finalizer. Newly nested views may depend on it from this point.
        PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            for frame in &mut stack.frames[first..end] {
                if frame.committed {
                    continue;
                }
                assert!(frame.complete && !frame.failed && frame.dependency >= first);
                frame.committed = true;
            }
        });
        for index in first..end {
            let candidate = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let frame = &stack.frames[index];
                if frame.pin_released {
                    return None;
                }
                Some((frame.bridge, frame.bits, frame.pointer))
            });
            let Some((bridge, bits, pointer)) = candidate else {
                continue;
            };
            if !pointer.is_null() {
                // Every recorded bridge is borrowed by the still-active outer
                // publication. A cross-bridge child can only remain provisional
                // by depending on that outer component.
                let bridge = unsafe { &*bridge };
                // Building still excludes a concurrent direct retirement. Drop
                // the pin before publishing Ready; an otherwise unowned view
                // may retire here, while every sibling still has its own pin.
                crate::api::errors::with_preserved_error(|| unsafe {
                    bridge.decref_publication_reference(pointer);
                });
                let mut handle = bridge.handle_shard(bits).lock();
                if let Some(entry) = handle.to_py.get_mut(&bits) {
                    assert_eq!(entry.view.py_obj(), pointer);
                    assert!(
                        matches!(&entry.publication, PublicationState::Building { owner } if *owner == std::thread::current().id())
                    );
                    entry.publication = PublicationState::Ready;
                }
            }
            PUBLICATION_BUILD_STACK
                .with(|stack| stack.borrow_mut().frames[index].pin_released = true);
        }
        // Wake waiters after all surviving members have released their pins.
        for index in first..end {
            let (bridge, bits, pointer) = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let frame = &stack.frames[index];
                (frame.bridge, frame.bits, frame.pointer)
            });
            if pointer.is_null() {
                continue;
            }
            let bridge = unsafe { &*bridge };
            bridge.publication_ready[bridge.handle_shard_index(bits)].notify_all();
        }
    }

    /// Roll back every remaining dependent frame. Independent completed views
    /// (including normalized error objects) have already committed and survive.
    /// No allocation is required after failure: the DFS records own retirement.
    pub(super) fn finish_publication_stack(&self) {
        PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            assert!(stack.active.is_empty());
            stack.rolling_back = true;
        });
        let mut closed = 0;
        let mut drained = 0;
        loop {
            let next = PUBLICATION_BUILD_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                // Close every sibling before the first decref. Callbacks may
                // append frames; each appended frame is closed/drained once.
                while closed < stack.frames.len() {
                    let frame = &mut stack.frames[closed];
                    if !frame.committed {
                        frame.aborting = true;
                    }
                    closed += 1;
                }
                while drained < stack.frames.len() {
                    let frame = &mut stack.frames[drained];
                    drained += 1;
                    if !frame.committed && !frame.edges_cleared {
                        frame.edges_cleared = true;
                        return Some((frame.bridge, frame.bits, frame.pointer));
                    }
                }
                None
            });
            let Some((bridge, bits, pointer)) = next else {
                break;
            };
            if pointer.is_null() {
                continue;
            }
            let bridge = unsafe { &*bridge };
            let entry = {
                let mut handle = bridge.handle_shard(bits).lock();
                let entry = handle
                    .to_py
                    .get_mut(&bits)
                    .expect("rollback lost a pinned view");
                assert_eq!(entry.view.py_obj(), pointer);
                &raw mut **entry
            };
            // The runtime GIL and Building state exclude other users. Keep all
            // entries in both identity maps, with their C pins, until every
            // owned edge has been broken. Internal decrefs therefore still use
            // canonical lifecycle custody and can never free a sibling view.
            unsafe {
                (*entry).view.release_owned_items_with(|edge, mirrored| {
                    if mirrored {
                        bridge.projection_unadopt_owned_ref(edge);
                    }
                    bridge.decref_publication_reference(edge);
                });
            }
        }
        let end = PUBLICATION_BUILD_STACK.with(|stack| stack.borrow().frames.len());
        for index in 0..end {
            let candidate = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                let frame = &stack.frames[index];
                (!frame.committed && !frame.pointer.is_null()).then_some((
                    frame.bridge,
                    frame.bits,
                    frame.pointer,
                ))
            });
            let Some((bridge, bits, pointer)) = candidate else {
                continue;
            };
            let bridge = unsafe { &*bridge };
            // Every private incoming owner must be in this retiring component.
            // Check while the allocation and canonical identity are still live;
            // a leftover mirror would become a dangling pointer after removal.
            assert_eq!(
                bridge.mirrored_c_refcount(pointer.addr()),
                0,
                "retiring projection still has an incoming mirrored owner"
            );
            let (mut address, mut handle) = bridge.lock_address_then_handle(pointer.addr(), bits);
            let mut entry = handle
                .to_py
                .remove(&bits)
                .expect("projection retirement identity disappeared");
            assert_eq!(entry.view.py_obj(), pointer);
            entry.publication = PublicationState::Retiring;
            address.from_py.remove(&pointer.addr());
            address.direct_molt_py.remove(&pointer.addr());
            drop(handle);
            drop(address);
            PUBLICATION_BUILD_STACK
                .with(|stack| stack.borrow_mut().frames[index].retired = Some(entry));
        }
        // Clear all old view marks before permitting a retry to publish a new
        // identity. Every old allocation is still alive and already edge-free.
        for index in 0..end {
            let bits = PUBLICATION_BUILD_STACK.with(|stack| {
                let stack = stack.borrow();
                stack.frames[index].retired.as_ref().map(|entry| entry.bits)
            });
            if let Some(bits) = bits {
                unsafe {
                    (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 0);
                }
            }
        }
        let mut frames = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            assert!(stack.active.is_empty());
            stack.rolling_back = false;
            std::mem::take(&mut stack.frames)
        });
        // Drop physical allocations only after the whole component is detached.
        for frame in &mut frames {
            drop(frame.retired.take());
        }
        for frame in frames {
            if frame.committed || frame.pointer.is_null() {
                continue;
            }
            let bridge = unsafe { &*frame.bridge };
            bridge.publication_ready[bridge.handle_shard_index(frame.bits)].notify_all();
            unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(frame.bits);
            }
        }
    }

    /// The sole managed-view insertion transaction. A rejected entry is
    /// returned unpublished; the caller unwinds it after both locks drop.
    pub(super) fn insert_managed_entry(
        &self,
        bits: AbiHandle,
        entry: Box<BridgeEntry>,
    ) -> Result<(), (Box<BridgeEntry>, ManagedEntryRejection)> {
        let addr = entry.view.py_obj().addr();
        let (mut address, mut handle) = self.lock_address_then_handle(addr, bits);
        if handle.to_py.contains_key(&bits) || handle.raw_py.contains_key(&bits) {
            return Err((entry, ManagedEntryRejection::Occupied));
        }
        if address.from_py.try_reserve(1).is_err() || handle.to_py.try_reserve(1).is_err() {
            return Err((entry, ManagedEntryRejection::NoMemory));
        }
        if unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 1) } == 0 {
            return Err((entry, ManagedEntryRejection::Deallocating));
        }
        address.from_py.insert(addr, bits);
        handle.to_py.insert(bits, entry);
        Ok(())
    }
}

// Canonical runtime type namespace projection and its physical ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeTypeProjection {
    /// Runtime declarations own slots, independently of whether physical
    /// storage is a managed view or an exported process-owned compact shell.
    RuntimeSlots,
    Native,
}

impl ObjectBridge {
    /// Slot authority is distinct from allocation ownership. Both compact
    /// managed views and declared runtime-backed process shells use the same
    /// namespace-to-slot resolver; only native declarations supply C slots.
    pub(crate) fn type_uses_runtime_slots(&self, pointer: *mut PyTypeObject) -> bool {
        self.managed_type_storage(pointer).is_some()
            || crate::api::typeobj::process_runtime_protocols(pointer).is_some()
    }

    /// Observe every published class projection without creating a C view.
    /// Noncanonical process bindings have only address-side entries.
    pub fn type_has_projection(&self, bits: AbiHandle) -> bool {
        {
            let handle = self.handle_shard(bits).lock();
            if handle.to_py.contains_key(&bits) || handle.raw_py.contains_key(&bits) {
                return true;
            }
        }
        self.address_shards.iter().any(|shard| {
            shard
                .lock()
                .direct_molt_py
                .values()
                .any(|bound| *bound == bits)
        })
    }

    /// Retain the existing canonical view and every admitted direct type alias.
    /// This observes the bridge's binding authority without creating a C view
    /// or a reverse registry. Collect addresses under their locks, then retain
    /// owners after releasing all locks, before callback-capable publication.
    /// Canonical and direct bindings may designate the same pointer.
    ///
    /// # Safety
    /// The caller holds the runtime GIL and a live runtime class reference.
    /// Direct type bindings obey static admission's exact identity and
    /// storage-lifetime contract through collection and retention.
    pub unsafe fn existing_type_projections(
        &self,
        bits: AbiHandle,
    ) -> Result<Vec<crate::api::refcount::OwnedPyObject>, ()> {
        unsafe {
            let mut roots = Vec::new();
            if let Some(pointer) = self.published_pyobj(bits, true) {
                // A recursive publication rejection is not an absent view.
                // Its original error must survive every observer of the cohort.
                if pointer.is_null() {
                    ensure_result_error(c"managed type projection is retiring");
                    return Err(());
                }
                roots.push(crate::api::refcount::OwnedPyObject::from_owned(pointer));
            }
            let mut aliases = Vec::new();
            for shard in &self.address_shards {
                let address = shard.lock();
                aliases.extend(
                    address
                        .direct_molt_py
                        .iter()
                        .filter_map(|(&pointer, &bound)| (bound == bits).then_some(pointer)),
                );
            }
            aliases.sort_unstable();
            aliases.dedup();
            for alias in aliases {
                if roots.iter().any(|root| root.as_ptr().addr() == alias) {
                    continue;
                }
                let pointer = core::ptr::with_exposed_provenance_mut::<PyObject>(alias);
                roots.push(crate::api::refcount::OwnedPyObject::from_borrowed(pointer));
            }
            Ok(roots)
        }
    }

    /// Publish mutable semantic flags to every existing projection, including
    /// process-owned aliases of either semantic origin. This never creates a
    /// C view. Semantic heap origin does not exclude direct aliases: runtime
    /// bootstrap binds heap exception classes, and native alias admission also
    /// permits them. Collection scans the existing direct bindings and sorts
    /// matching roots for deduplication; transient ownership is bounded by that
    /// cohort, with no reverse registry.
    ///
    /// Static binding publishes direct_molt_py only after the runtime supplies
    /// the exact semantic handle for a process-owned C object. For a handle
    /// classified as Type, matching bindings therefore denote PyTypeObject
    /// aliases, not arbitrary instance projections. The caller's live class
    /// reference and runtime GIL preserve this binding cohort. The shared
    /// projection collector retains every existing root outside address locks.
    /// The unsafe static-binding admission contract requires pointer
    /// storage to outlive its exact binding; dynamic managed instance views use
    /// different bridge maps and cannot be selected by this type-handle scan.
    ///
    /// # Safety
    /// The caller holds the runtime GIL and a live managed type reference; mask
    /// includes only semantic flags owned by that runtime class. Every direct
    /// binding for bits must obey static-binding admission's exact identity and
    /// storage-lifetime contract.
    pub unsafe fn publish_existing_type_flags(
        &self,
        bits: AbiHandle,
        mask: std::os::raw::c_ulong,
        flags: std::os::raw::c_ulong,
    ) -> Result<(), ()> {
        unsafe {
            assert_eq!(
                (crate::hooks::hooks_or_stubs().classify_heap)(bits),
                MoltTypeTag::Type as u8,
                "semantic flags require a live managed type handle"
            );
            let roots = self.existing_type_projections(bits)?;
            for root in &roots {
                let tp = root.as_ptr().cast::<PyTypeObject>();
                (*tp).tp_flags = ((*tp).tp_flags & !mask) | (flags & mask);
            }
            Ok(())
        }
    }

    /// Some(None) is a proven compact managed allocation; it must never fall
    /// through to a foreign or flag-derived storage guess.
    pub(crate) fn managed_type_storage(
        &self,
        pointer: *mut PyTypeObject,
    ) -> Option<Option<*mut crate::abi_types::PyHeapTypeObject>> {
        let bits = self.managed_handle_for_pyobj(pointer.cast())?;
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        if entry.view.py_obj() != pointer.cast() {
            return None;
        }
        Some(match &entry.view {
            ManagedView::Type { object, .. } => object.heap(),
            _ => None,
        })
    }

    pub(super) fn refresh_type_view(&self, bits: AbiHandle) -> bool {
        let pointer = {
            let handle = self.handle_shard(bits).lock();
            let Some(entry) = handle.to_py.get(&bits) else {
                return true;
            };
            let ManagedView::Type { object, .. } = &entry.view else {
                return true;
            };
            object.get()
        };
        if !unsafe { self.publish_type_state(bits, pointer, true) } {
            return false;
        }
        PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(index) = stack.find(self, bits) {
                stack.frames[index].physical_type_pending = true;
            }
        });
        true
    }

    /// Explicit C namespace exposure. Managed Type views mirror a runtime
    /// graph edge; process-owned static shells own a normal C root retired by
    /// retire_static_type_runtime_roots. Neither lane copies dictionary data.
    pub(crate) unsafe fn expose_runtime_type_dictionary(
        &self,
        pointer: *mut PyTypeObject,
    ) -> Result<Option<RuntimeTypeProjection>, ()> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let Some(value) = self.molt_handle_for_pyobj(pointer.cast()) else {
            return Ok(None);
        };
        let bits = value.bits();
        if unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits) }
            != MoltTypeTag::Type as u8
        {
            return Ok(None);
        }
        let mirrored = {
            let handle = self.handle_shard(bits).lock();
            handle.to_py.get(&bits).is_some_and(|entry| {
                entry.view.py_obj() == pointer.cast()
                    && matches!(&entry.view, ManagedView::Type { .. })
            })
        };
        if unsafe { self.publish_type_state(bits, pointer, mirrored) } {
            Ok(Some(if self.type_uses_runtime_slots(pointer) {
                RuntimeTypeProjection::RuntimeSlots
            } else {
                RuntimeTypeProjection::Native
            }))
        } else {
            Err(())
        }
    }

    // Address and handle custody must describe this exact retained allocation.
    // Noncanonical static bindings intentionally have no raw_py reverse entry.
    pub(super) fn type_projection_matches(
        address: &AddressShard,
        handle: &HandleShard,
        bits: AbiHandle,
        pointer: *mut PyTypeObject,
        heap: Option<*mut crate::abi_types::PyHeapTypeObject>,
        mirrored: bool,
    ) -> bool {
        if address.from_py.get(&pointer.addr()) != Some(&bits) {
            return false;
        }
        if mirrored {
            handle.to_py.get(&bits).is_some_and(|entry| {
                entry.view.py_obj() == pointer.cast()
                    && matches!(&entry.view, ManagedView::Type { object, .. } if object.heap() == heap)
            })
        } else {
            address.direct_molt_py.get(&pointer.addr()) == Some(&bits)
                && crate::abi_types::process_heap_type_storage(pointer) == heap
        }
    }

    /// Runtime C3 order is semantic authority; a physical type's MRO must
    /// nevertheless begin with that exact C type. A noncanonical direct alias
    /// owns a separate tuple with its own head and the unchanged canonical
    /// tail. Never mutate the runtime tuple or replace its reverse projection.
    unsafe fn physical_type_mro(
        &self,
        bits: AbiHandle,
        pointer: *mut PyTypeObject,
        mro: crate::api::refcount::OwnedPyObject,
    ) -> Option<crate::api::refcount::OwnedPyObject> {
        use crate::api::refcount::{OwnedPyObject, Py_INCREF};
        use crate::api::sequences::{PyTuple_GetItem, PyTuple_New, PyTuple_SetItem, PyTuple_Size};
        unsafe {
            let count = PyTuple_Size(mro.as_ptr());
            if count <= 0 {
                ensure_result_error(c"runtime type MRO has no self entry");
                return None;
            }
            let head = PyTuple_GetItem(mro.as_ptr(), 0);
            if !self.pyobj_matches_handle(head, bits) {
                ensure_result_error(c"runtime type MRO self identity mismatch");
                return None;
            }
            if head == pointer.cast() {
                return Some(mro);
            }
            let physical = OwnedPyObject::from_owned(PyTuple_New(count));
            if physical.as_ptr().is_null() {
                return None;
            }
            for index in 0..count {
                let entry = if index == 0 {
                    pointer.cast()
                } else {
                    PyTuple_GetItem(mro.as_ptr(), index)
                };
                if entry.is_null() {
                    ensure_result_error(c"runtime type MRO entry is missing");
                    return None;
                }
                Py_INCREF(entry);
                // SetItem consumes the new reference on both outcomes and
                // preserves exact admitted direct-alias projection identity.
                if PyTuple_SetItem(physical.as_ptr(), index, entry) < 0 {
                    return None;
                }
            }
            Some(physical)
        }
    }

    /// Stage the entire fixed owner inventory before publishing any C field.
    /// Raw process-shell names are allocated before hierarchy/dict publication.
    pub(super) unsafe fn publish_type_state(
        &self,
        bits: AbiHandle,
        pointer: *mut PyTypeObject,
        mirrored: bool,
    ) -> bool {
        use crate::api::refcount::OwnedPyObject;
        use crate::hooks::TypeMetadataField;
        unsafe {
            let _view_owner = OwnedPyObject::from_borrowed(pointer.cast());
            let heap = if mirrored {
                self.managed_type_storage(pointer).flatten()
            } else {
                crate::abi_types::process_heap_type_storage(pointer)
            };
            let mut values: [OwnedPyObject; 5] =
                std::array::from_fn(|_| OwnedPyObject::from_owned(std::ptr::null_mut()));
            let mut fields = [std::ptr::null_mut(); 5];
            let mut count = 0;
            for (field, destination) in [
                (TypeMetadataField::Bases, &raw mut (*pointer).tp_bases),
                (TypeMetadataField::Mro, &raw mut (*pointer).tp_mro),
            ] {
                let value = OwnedPyObject::from_owned(self.owned_result_to_pyobj(
                    (crate::hooks::hooks_or_stubs().type_metadata)(bits, field),
                ));
                if value.as_ptr().is_null() {
                    ensure_result_error(c"type metadata projection is missing");
                    return false;
                }
                values[count] = if !mirrored && field == TypeMetadataField::Mro {
                    let Some(value) = self.physical_type_mro(bits, pointer, value) else {
                        return false;
                    };
                    value
                } else {
                    value
                };
                fields[count] = destination;
                count += 1;
            }
            if let Some(heap) = heap {
                for (field, destination) in [
                    (TypeMetadataField::Name, &raw mut (*heap).ht_name),
                    (TypeMetadataField::QualName, &raw mut (*heap).ht_qualname),
                ] {
                    values[count] = OwnedPyObject::from_owned(self.owned_result_to_pyobj(
                        (crate::hooks::hooks_or_stubs().type_metadata)(bits, field),
                    ));
                    if values[count].as_ptr().is_null() {
                        ensure_result_error(c"type metadata projection is missing");
                        return false;
                    }
                    fields[count] = destination;
                    count += 1;
                }
            }
            let dictionary =
                self.borrowed_result_to_borrowed_pyobj((crate::hooks::hooks_or_stubs()
                    .type_dict_borrowed)(
                    bits
                ));
            if dictionary.is_null() {
                ensure_result_error(c"type dictionary projection is missing");
                return false;
            }
            values[count] = OwnedPyObject::from_borrowed(dictionary);
            fields[count] = &raw mut (*pointer).tp_dict;
            count += 1;
            let mut shell_name = OwnedTypeName(std::ptr::null_mut());
            if !mirrored
                && let Some(heap) = heap
                && (*heap)._ht_tpname.is_null()
            {
                let mut length = 0;
                let bytes = crate::api::strings::PyUnicode_AsUTF8AndSize(
                    values[2].as_ptr(),
                    &raw mut length,
                );
                if bytes.is_null() {
                    return false;
                }
                let Some(prepared) = OwnedTypeName::copy(bytes.cast(), length as usize) else {
                    return false;
                };
                shell_name = prepared;
            }
            let mut acquired = [std::ptr::null_mut(); 5];
            for index in 0..count {
                let value = values[index].as_ptr();
                if mirrored {
                    if !self.projection_incref(value) {
                        crate::api::errors::with_preserved_error(|| {
                            for root in acquired {
                                self.projection_decref(root);
                            }
                        });
                        return false;
                    }
                } else {
                    crate::api::refcount::Py_INCREF(value);
                }
                acquired[index] = value;
            }
            let mut previous = [std::ptr::null_mut(); 5];
            let committed = {
                let (address, handle) = self.lock_address_then_handle(pointer.addr(), bits);
                let same =
                    Self::type_projection_matches(&address, &handle, bits, pointer, heap, mirrored);
                if same {
                    for index in 0..count {
                        previous[index] = fields[index].replace(acquired[index]);
                    }
                    if let Some(heap) = heap
                        && !shell_name.0.is_null()
                    {
                        (*heap)._ht_tpname = shell_name.take();
                        (*pointer).tp_name = (*heap)._ht_tpname;
                    }
                }
                same
            };
            if !committed {
                ensure_result_error(c"type projection identity changed before publication");
            }
            crate::api::errors::with_preserved_error(|| {
                for root in if committed { previous } else { acquired } {
                    if mirrored {
                        self.projection_decref(root);
                    } else {
                        crate::api::refcount::Py_XDECREF(root);
                    }
                }
            });
            committed
        }
    }
}

// Canonical Molt-handle to CPython-view publication and result ownership.
impl ObjectBridge {
    pub(super) unsafe fn handle_to_pyobj_impl(
        &self,
        bits: AbiHandle,
        owned: bool,
    ) -> *mut PyObject {
        // Pin before any shard lock.  Runtime ownership, physical projection
        // population, and publication state form one bridge transaction.
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        if let Some(ptr) = Self::singleton_pyobj(bits) {
            return ptr;
        }
        if let Some(ptr) = unsafe { self.published_pyobj(bits, owned) } {
            if owned {
                unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
            }
            if ptr.is_null() {
                unsafe {
                    ensure_result_error(c"ABI publication is rolling back");
                }
            }
            return ptr;
        }

        // A cold physical view may materialize fields or a type namespace.
        // Incoming errors belong to the caller, not to those result hooks.
        // Preserve them across the complete publication and rollback, without
        // re-entering this decision through a possibly partial bootstrap hook.
        let publish = || unsafe { self.publish_cold_pyobj(bits, owned) };
        if crate::api::errors::raised_error_pending() {
            crate::api::errors::with_preserved_error(publish)
        } else {
            publish()
        }
    }

    /// Publish under the caller's runtime GIL and error-preservation boundary.
    pub(super) unsafe fn publish_cold_pyobj(&self, bits: AbiHandle, owned: bool) -> *mut PyObject {
        let Some(build_guard) = PublicationBuildGuard::enter(self, bits) else {
            if owned {
                crate::api::errors::with_preserved_error(|| unsafe {
                    (crate::hooks::hooks_or_stubs().dec_ref)(bits);
                });
            }
            return std::ptr::null_mut();
        };
        loop {
            // A borrowed crossing must manufacture the stable runtime hold;
            // an owned crossing transfers its incoming hold to the view.
            if !owned {
                unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(bits) };
            }
            let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
            let (initial_refs, has_non_view_runtime_owner) =
                initial_managed_view_refs(runtime_refs, owned);
            let pinned_refs = if crate::abi_types::is_immortal_refcnt(initial_refs) {
                initial_refs
            } else {
                initial_refs + 1
            };
            let Some((entry, raw_ptr)) =
                // One temporary C pin keeps even an otherwise unowned nested
                // view alive until its recursive component commits or unwinds.
                (unsafe { self.build_pyobj_entry(bits, pinned_refs, has_non_view_runtime_owner) })
            else {
                crate::api::errors::with_preserved_error(|| unsafe {
                    (crate::hooks::hooks_or_stubs().dec_ref)(bits)
                });
                drop(build_guard);
                unsafe {
                    ensure_result_error(c"managed runtime handle could not build an ABI view")
                };
                return std::ptr::null_mut();
            };
            match self.insert_managed_entry(bits, entry) {
                Ok(()) => build_guard.inserted(raw_ptr),
                Err((_, ManagedEntryRejection::Occupied)) => {
                    if !owned {
                        unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                    }
                    if let Some(ptr) = unsafe { self.published_pyobj(bits, owned) } {
                        if owned {
                            unsafe { (crate::hooks::hooks_or_stubs().dec_ref)(bits) };
                        }
                        if !build_guard.finish() {
                            return std::ptr::null_mut();
                        }
                        return ptr;
                    }
                    continue;
                }
                Err((_, rejection)) => {
                    crate::api::errors::with_preserved_error(|| unsafe {
                        (crate::hooks::hooks_or_stubs().dec_ref)(bits);
                    });
                    drop(build_guard);
                    unsafe { rejection.set_error() };
                    return std::ptr::null_mut();
                }
            }

            if !self.refresh_type_view(bits)
                || !self.refresh_tuple_view(bits)
                || !self.refresh_slice_view_with_origins(bits, None)
                || !self.refresh_list_view(bits)
                || !self.refresh_exception_view(bits)
                || !self.refresh_memoryview_view(bits)
            {
                drop(build_guard);
                unsafe {
                    ensure_result_error(c"managed runtime handle could not populate an ABI view")
                };
                return std::ptr::null_mut();
            }
            if !build_guard.finish() {
                unsafe {
                    ensure_result_error(c"recursive ABI view publication failed");
                }
                return std::ptr::null_mut();
            }
            return raw_ptr;
        }
    }

    /// Translate a Molt handle to a new-reference `PyObject*`.
    pub unsafe fn owned_handle_to_pyobj(&self, bits: AbiHandle) -> *mut PyObject {
        unsafe { self.handle_to_pyobj_impl(bits, true) }
    }

    /// Translate a Molt handle to a borrowed `PyObject*`.
    pub unsafe fn handle_to_borrowed_pyobj(&self, bits: AbiHandle) -> *mut PyObject {
        unsafe { self.handle_to_pyobj_impl(bits, false) }
    }

    /// Acquire a new C reference without consuming the caller's runtime owner.
    /// All borrowed runtime arguments use this crossing, including foreign
    /// slot names, values, positional tuples, and keyword dictionaries.
    pub unsafe fn borrowed_handle_to_new_pyobj(&self, bits: AbiHandle) -> *mut PyObject {
        unsafe { (crate::hooks::hooks_or_stubs().inc_ref)(bits) };
        unsafe { self.owned_handle_to_pyobj(bits) }
    }

    pub unsafe fn owned_result_to_pyobj(
        &self,
        result: crate::hooks::OwnedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::raised_error_pending() {
                    crate::api::errors::with_preserved_error(|| unsafe {
                        (crate::hooks::hooks_or_stubs().dec_ref)(bits)
                    });
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime owned-result hook returned a value with an exception set",
                        )
                    };
                    std::ptr::null_mut()
                } else {
                    unsafe { self.owned_handle_to_pyobj(bits) }
                }
            }
            crate::hooks::DecodedHandleResult::Missing => {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                std::ptr::null_mut()
            }
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    ensure_result_error(c"runtime owned-result hook failed without an exception")
                };
                std::ptr::null_mut()
            }
        }
    }
}

// Borrowed/new-result projection ownership at the C ABI boundary.
impl ObjectBridge {
    pub unsafe fn borrowed_result_to_borrowed_pyobj(
        &self,
        result: crate::hooks::BorrowedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::raised_error_pending() {
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime borrowed-result hook returned a value with an exception set",
                        )
                    };
                    std::ptr::null_mut()
                } else {
                    let ptr = unsafe { self.handle_to_borrowed_pyobj(bits) };
                    if ptr.is_null() {
                        unsafe {
                            ensure_result_error(
                                c"runtime borrowed-result handle could not enter the bridge",
                            )
                        };
                    }
                    ptr
                }
            }
            crate::hooks::DecodedHandleResult::Missing => {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                std::ptr::null_mut()
            }
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    ensure_result_error(c"runtime borrowed-result hook failed without an exception")
                };
                std::ptr::null_mut()
            }
        }
    }

    pub unsafe fn borrowed_result_to_new_pyobj(
        &self,
        result: crate::hooks::BorrowedHandleResult,
    ) -> *mut PyObject {
        match result.decode() {
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                if crate::api::errors::raised_error_pending() {
                    unsafe {
                        crate::api::errors::replace_current_with_system_error(
                            "runtime borrowed-result hook returned a value with an exception set",
                        )
                    };
                    return std::ptr::null_mut();
                }
                let ptr = unsafe { self.borrowed_handle_to_new_pyobj(bits) };
                if ptr.is_null() {
                    unsafe {
                        ensure_result_error(
                            c"runtime borrowed-result handle could not enter the bridge",
                        )
                    };
                    return std::ptr::null_mut();
                }
                ptr
            }
            crate::hooks::DecodedHandleResult::Missing => {
                let _ = crate::api::errors::transfer_runtime_pending_to_current();
                std::ptr::null_mut()
            }
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    ensure_result_error(c"runtime borrowed-result hook failed without an exception")
                };
                std::ptr::null_mut()
            }
        }
    }
}

/// Clinic's leading name/signature plus separator is metadata, not public
/// doc text. Non-Clinic documentation remains byte-for-byte unchanged.
fn cfunction_clinic_document<'a>(name: &[u8], document: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    let suffix = document.strip_prefix(name)?;
    if suffix.first() != Some(&b'(') {
        return None;
    }
    let end = suffix.windows(5).position(|window| window == b"\n--\n\n")?;
    let signature = &suffix[..end];
    if signature.last() != Some(&b')') {
        return None;
    }
    Some((signature, &suffix[end + 5..]))
}

// Concrete C callable publication and GC traversal.
impl ObjectBridge {
    /// Runtime callable carriers have actual vectorcall storage even though
    /// their physical header intentionally does not claim a native C function.
    pub(crate) fn runtime_vectorcall(
        &self,
        object: *mut PyObject,
    ) -> Option<crate::abi_types::PyVectorcallFunc> {
        let bits = self.managed_handle_for_pyobj(object)?;
        let handle = self.handle_shard(bits).lock();
        let ManagedView::RuntimeCallable(callable) = &handle.to_py.get(&bits)?.view else {
            return None;
        };
        unsafe { (*callable.get()).func.vectorcall }
    }

    /// Publish a fresh runtime C callable as its canonical concrete view and
    /// return one new C reference. `object` is the complete
    /// `PyCFunctionObject`/`PyCMethodObject` layout; its `m_self`, `m_module`
    /// and `mm_class` are borrowed from the caller and adopted here.
    ///
    /// The owned runtime `bits` becomes the stable view hold on success and is
    /// released on failure. Releasing the constructor C reference therefore
    /// retires nothing while any runtime owner (for example a module dict)
    /// remains; the physical object, `m_ml` and all member edges persist until
    /// runtime terminal retirement runs `release_owned_items`. `m_self` and
    /// `mm_class` mirror the runtime callable closure, so like clean tuple
    /// items they are untraversed projection edges and a module/function
    /// cycle stays collectable. `m_module` has no runtime mirror and is an
    /// independent edge exposed by `visit_physical_owned_edges_for_gc`.
    pub unsafe fn publish_cfunction_view(
        &self,
        bits: AbiHandle,
        object: PyCMethodObject,
    ) -> *mut PyObject {
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        let Some(build_guard) = PublicationBuildGuard::enter(self, bits) else {
            crate::api::errors::with_preserved_error(|| unsafe {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            });
            return std::ptr::null_mut();
        };
        let release_entry = |entry: &mut BridgeEntry| {
            crate::api::errors::with_preserved_error(|| unsafe {
                entry.view.release_owned_items();
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            })
        };
        let edges = [
            (object.func.m_self, true),
            (object.func.m_module, false),
            (object.mm_class.cast::<PyObject>(), true),
        ];
        let runtime_refs = unsafe { (crate::hooks::hooks_or_stubs().ref_count)(bits) };
        let (initial_refs, has_non_view_runtime_owner) =
            initial_managed_view_refs(runtime_refs, true);
        let mut object = object;
        object.func.ob_base.ob_refcnt = if crate::abi_types::is_immortal_refcnt(initial_refs) {
            initial_refs
        } else {
            initial_refs + 1
        };
        object.func.m_self = std::ptr::null_mut();
        object.func.m_module = std::ptr::null_mut();
        object.mm_class = std::ptr::null_mut();
        let mut entry = Box::new(BridgeEntry {
            view: ManagedView::CFunction(Box::new(UnsafeCell::new(object))),
            bits,
            unicode: None,
            publication: PublicationState::Building {
                owner: std::thread::current().id(),
            },
            lifecycle: if has_non_view_runtime_owner {
                BridgeLifecycle::RuntimeOwned
            } else {
                BridgeLifecycle::ViewHoldOnly
            },
        });
        let ptr = entry.view.py_obj();
        let physical = ptr.cast::<PyCMethodObject>();
        let slots = unsafe {
            [
                &raw mut (*physical).func.m_self,
                &raw mut (*physical).func.m_module,
                (&raw mut (*physical).mm_class).cast::<*mut PyObject>(),
            ]
        };
        for (slot, (edge, mirrored)) in slots.into_iter().zip(edges) {
            // These C fields arrive as raw borrowed pointers, so their existing
            // provisional dependencies need not pass through published_pyobj.
            // Retaining an opaque field must not commit its mutable projection
            // or require an open construction container to be observable.
            if let Some(value) = self.managed_handle_for_pyobj(edge)
                && !observe_publication(self, value)
            {
                release_entry(&mut entry);
                return std::ptr::null_mut();
            }
            if mirrored {
                if !unsafe { self.projection_incref(edge) } {
                    release_entry(&mut entry);
                    return std::ptr::null_mut();
                }
            } else {
                unsafe { crate::api::refcount::Py_XINCREF(edge) };
            }
            // Rollback owns only slots whose retain completed successfully.
            unsafe { *slot = edge };
        }
        match self.insert_managed_entry(bits, entry) {
            Ok(()) => {
                build_guard.inserted(ptr);
                if build_guard.finish() {
                    ptr
                } else {
                    unsafe {
                        ensure_result_error(c"recursive C callable publication failed");
                    }
                    std::ptr::null_mut()
                }
            }
            Err((mut entry, rejection)) => {
                release_entry(&mut entry);
                unsafe { rejection.set_error() };
                std::ptr::null_mut()
            }
        }
    }

    /// CPython's internal Clinic document owns both public documentation and
    /// text signature. Copy bytes only while borrowing its canonical C view.
    pub fn cfunction_metadata(&self, bits: AbiHandle, signature: bool) -> Option<Option<Vec<u8>>> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let handle = self.handle_shard(bits).lock();
        let entry = handle.to_py.get(&bits)?;
        let ManagedView::CFunction(object) = &entry.view else {
            return None;
        };
        let method = unsafe { (*object.get()).func.m_ml.as_ref() }?;
        if method.ml_doc.is_null() {
            return Some(None);
        }
        let document = unsafe { std::ffi::CStr::from_ptr(method.ml_doc).to_bytes() };
        let name = (!method.ml_name.is_null())
            .then(|| unsafe { std::ffi::CStr::from_ptr(method.ml_name).to_bytes() });
        let parsed = name.and_then(|name| cfunction_clinic_document(name, document));
        Some(if signature {
            parsed.map(|(signature, _)| signature.to_vec())
        } else {
            Some(parsed.map_or(document, |(_, body)| body).to_vec())
        })
    }

    /// The receiver belongs to the C method/context, never to a second public
    /// function-dictionary edge. The returned runtime handle is owned.
    pub fn cfunction_self(&self, bits: AbiHandle) -> Option<Result<AbiHandle, ()>> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let receiver = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            let ManagedView::CFunction(object) = &entry.view else {
                return None;
            };
            unsafe { (*object.get()).func.m_self }
        };
        Some(if receiver.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            unsafe { RuntimeValue::acquire(receiver) }
                .map(RuntimeValue::into_owned_bits)
                .ok_or(())
        })
    }

    /// The physical C member is the sole module-metadata authority for these
    /// callables. None means this value has no CFunction view; an error keeps
    /// the C exception intact for the runtime boundary to transfer.
    pub fn cfunction_module(&self, bits: AbiHandle) -> Option<Result<AbiHandle, ()>> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        let module = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle.to_py.get(&bits)?;
            let ManagedView::CFunction(object) = &entry.view else {
                return None;
            };
            unsafe { (*object.get()).func.m_module }
        };
        Some(if module.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            unsafe { RuntimeValue::acquire(module) }
                .map(RuntimeValue::into_owned_bits)
                .ok_or(())
        })
    }

    /// Replace a CFunction's module edge transactionally. Deletion clears the
    /// physical pointer (reads produce None); assigning None retains Py_None.
    /// New ownership is staged before publication; old ownership retires only
    /// after the handle lock is released, since finalizers may re-enter.
    pub fn set_cfunction_module(&self, bits: AbiHandle, value: Option<AbiHandle>) -> Option<bool> {
        let _gil = crate::hooks::RuntimeGilGuard::ensure();
        {
            let handle = self.handle_shard(bits).lock();
            if !matches!(&handle.to_py.get(&bits)?.view, ManagedView::CFunction(_)) {
                return None;
            }
        }
        let pointer = if let Some(value) = value {
            let pointer = unsafe { self.borrowed_handle_to_new_pyobj(value) };
            if pointer.is_null() {
                return Some(false);
            }
            pointer
        } else {
            std::ptr::null_mut()
        };
        let old = {
            let handle = self.handle_shard(bits).lock();
            let entry = handle
                .to_py
                .get(&bits)
                .expect("caller retains CFunction owner");
            let ManagedView::CFunction(object) = &entry.view else {
                unreachable!()
            };
            unsafe { std::mem::replace(&mut (*object.get()).func.m_module, pointer) }
        };
        drop(RetiredOwnedCFields::one(old));
        Some(true)
    }
}

impl ObjectBridge {
    pub(super) fn classify_handle(bits: AbiHandle) -> MoltTypeTag {
        let obj = MoltObject::from_bits(bits);
        if obj.is_none() {
            return MoltTypeTag::None;
        }
        if obj.is_bool() {
            return MoltTypeTag::Bool;
        }
        if obj.is_int() {
            return MoltTypeTag::Int;
        }
        if obj.is_float() {
            return MoltTypeTag::Float;
        }
        if obj.is_ptr() {
            let hooks = crate::hooks::hooks_or_stubs();
            let tag = unsafe { (hooks.classify_heap)(bits) };
            match tag {
                value if value == MoltTypeTag::BuiltinCallable as u8 => {
                    MoltTypeTag::BuiltinCallable
                }
                value if value == MoltTypeTag::Int as u8 => MoltTypeTag::Int,
                value if value == MoltTypeTag::Complex as u8 => MoltTypeTag::Complex,
                value if value == MoltTypeTag::Str as u8 => MoltTypeTag::Str,
                value if value == MoltTypeTag::Bytes as u8 => MoltTypeTag::Bytes,
                value if value == MoltTypeTag::MemoryView as u8 => MoltTypeTag::MemoryView,
                value if value == MoltTypeTag::Slice as u8 => MoltTypeTag::Slice,
                value if value == MoltTypeTag::List as u8 => MoltTypeTag::List,
                value if value == MoltTypeTag::Tuple as u8 => MoltTypeTag::Tuple,
                value if value == MoltTypeTag::Dict as u8 => MoltTypeTag::Dict,
                value if value == MoltTypeTag::Set as u8 => MoltTypeTag::Set,
                value if value == MoltTypeTag::FrozenSet as u8 => MoltTypeTag::FrozenSet,
                value if value == MoltTypeTag::Type as u8 => MoltTypeTag::Type,
                value if value == MoltTypeTag::Module as u8 => MoltTypeTag::Module,
                value if value == MoltTypeTag::Traceback as u8 => MoltTypeTag::Traceback,
                value if value == MoltTypeTag::Exception as u8 => MoltTypeTag::Exception,
                _ => MoltTypeTag::Other,
            }
        } else {
            MoltTypeTag::Other
        }
    }
}

impl ObjectBridge {
    /// Adopt selected ready views into the existing closed publication transaction.
    /// Reserve all frames before pinning; reentrant callers leave the edge drain
    /// with the stack's current outer owner.
    pub(super) fn retire_publication_component(
        &self,
        component: impl ExactSizeIterator<Item = (AbiHandle, *mut PyObject)>,
    ) {
        // Reserve the complete transaction before changing an identity or
        // acquiring a pin. The GIL excludes mutation throughout the snapshot.
        let drain_now = PUBLICATION_BUILD_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let drain_now =
                stack.frames.is_empty() && stack.active.is_empty() && !stack.rolling_back;
            stack.frames.reserve(component.len());
            drain_now
        });
        for (bits, pointer) in component {
            let mut handle = self.handle_shard(bits).lock();
            let entry = handle
                .to_py
                .get_mut(&bits)
                .expect("retiring view disappeared");
            assert_eq!(entry.view.py_obj(), pointer);
            assert_eq!(entry.publication, PublicationState::Ready);
            let refs = unsafe { (*pointer).ob_refcnt };
            if !crate::abi_types::is_immortal_refcnt(refs) {
                let pinned = checked_c_ref_increment(refs).unwrap_or_else(|| {
                    abort_refcount_invariant(
                        "runtime projection retirement pin",
                        refs,
                        entry.lifecycle,
                    )
                });
                unsafe { (*pointer).ob_refcnt = pinned };
            }
            // Adopt the live view into the same closed transaction state used
            // by failed recursive publication. Decrefs still resolve through
            // the canonical map; observe_publication rejects new observers.
            entry.publication = PublicationState::Building {
                owner: std::thread::current().id(),
            };
            PUBLICATION_BUILD_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                let dependency = stack.frames.len();
                stack.frames.push(PublicationFrame {
                    bridge: self,
                    bits: bits,
                    pointer: pointer,
                    dependency,
                    complete: false,
                    physical_type_pending: false,
                    failed: true,
                    committed: false,
                    pin_released: false,
                    aborting: true,
                    edges_cleared: false,
                    retired: None,
                });
            });
        }
        // Reentrant retirement can retire a disjoint component while a
        // publication/retirement already owns this stack. Its existing outer
        // transaction drains the appended closed frames; their pins keep every
        // allocation alive in the meantime.
        if drain_now {
            crate::api::errors::with_preserved_error(|| self.finish_publication_stack());
        }
    }
}

#[cfg(test)]
mod bridge_publication_race_tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn same_handle_crossing_publishes_one_ready_pointer() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(20_000).bits();
        let workers = 8;
        let barrier = Arc::new(Barrier::new(workers));
        let (tx, rx) = mpsc::channel();
        for _ in 0..workers {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            thread::spawn(move || {
                barrier.wait();
                let ptr = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
                tx.send(ptr.addr()).expect("same-handle receiver dropped");
            });
        }
        let pointers = (0..workers)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<Vec<_>>();
        assert!(pointers.iter().all(|ptr| *ptr == pointers[0]));
        let handle = bridge.handle_shard(bits).lock();
        assert!(matches!(
            handle.to_py.get(&bits).map(|entry| &entry.publication),
            Some(PublicationState::Ready)
        ));
        drop(handle);
        let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(pointers[0]);
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    fn install_building_view(
        bridge: &ObjectBridge,
        bits: AbiHandle,
    ) -> (*mut PyObject, PublicationBuildGuard<'_>) {
        let guard = PublicationBuildGuard::enter(bridge, bits).unwrap();
        let (entry, ptr) = unsafe { bridge.build_pyobj_entry(bits, 2, false) }.unwrap();
        bridge
            .insert_managed_entry(bits, entry)
            .map_err(|(_, error)| error)
            .unwrap();
        guard.inserted(ptr);
        (ptr, guard)
    }

    #[test]
    fn building_publication_blocks_other_threads_until_ready() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(30_000).bits();
        let (ptr, guard) = install_building_view(&bridge, bits);
        // The owner may re-enter to materialize recursive projections.
        assert_eq!(
            bridge.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle),
            Some(bits)
        );
        let (tx, rx) = mpsc::channel();
        let waiter = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(waiter.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle))
                .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(guard.finish());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(bits));
        assert_eq!(bridge.release_pyobj(ptr), PyObjRelease::ManagedViewRetired);
    }

    #[test]
    fn publication_rollback_wakes_waiters_and_removes_both_directions() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(40_000).bits();
        let (ptr, guard) = install_building_view(&bridge, bits);
        let (tx, rx) = mpsc::channel();
        let waiter = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(waiter.pyobj_to_handle(ptr).map(BridgeIdentity::as_handle))
                .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(guard);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        assert!(
            !bridge
                .address_shard(addr)
                .lock()
                .from_py
                .contains_key(&addr)
        );
        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
    }

    #[test]
    fn release_waits_for_building_projection_before_retirement() {
        init_tag_table();
        let bridge = Arc::new(ObjectBridge::new());
        let bits = MoltObject::from_int(45_000).bits();
        let (ptr, guard) = install_building_view(&bridge, bits);
        // A direct-ingress hint must not bypass managed publication custody.
        bridge
            .address_shard(ptr.addr())
            .lock()
            .direct_molt_py
            .insert(ptr.addr(), bits);
        let (tx, rx) = mpsc::channel();
        let releaser = Arc::clone(&bridge);
        let addr = ptr.addr();
        thread::spawn(move || {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            tx.send(releaser.release_pyobj(ptr)).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(guard.finish());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PyObjRelease::ManagedViewRetired
        );
        assert!(
            !bridge
                .address_shard(addr)
                .lock()
                .from_py
                .contains_key(&addr)
        );
        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
    }
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;

#[cfg(test)]
mod handle_tests {
    use super::*;
    #[test]
    fn managed_type_retirement_and_rollback_revoke_subclass_identity() {
        use crate::api::typeobj::subclass_registry_tests::{
            assert_type_address_retired, register_subclass_for_test,
        };

        for allocated_heap in [false, true] {
            for exit in ["drop", "callbacks", "release", "deferred", "rollback"] {
                let mut base: PyTypeObject = unsafe { std::mem::zeroed() };
                let mut child: PyTypeObject = unsafe { std::mem::zeroed() };
                let base_pointer = &raw mut base;
                let child_pointer = &raw mut child;
                let mut prefix: PyTypeObject = unsafe { std::mem::zeroed() };
                prefix.ob_base.ob_base.ob_refcnt = 1;
                if allocated_heap {
                    prefix.tp_flags = crate::abi_types::Py_TPFLAGS_HEAPTYPE;
                }
                let mut view = ManagedView::Type {
                    object: ManagedTypeAllocation::new(prefix),
                    _name: std::ffi::CString::new("retiring.Type").unwrap(),
                };
                let pointer = view.py_obj().cast::<PyTypeObject>();
                let address = pointer.addr();
                unsafe {
                    register_subclass_for_test(base_pointer, pointer);
                    register_subclass_for_test(pointer, child_pointer);
                    // Logical flags cannot change the physical retirement owner.
                    (*pointer).tp_flags ^= crate::abi_types::Py_TPFLAGS_HEAPTYPE;
                }
                match exit {
                    "drop" => drop(view),
                    "callbacks" => {
                        let mut callbacks = 0;
                        view.release_owned_items_with(|_, _| {
                            assert_type_address_retired(address);
                            callbacks += 1;
                            if callbacks == 5 {
                                // A last release can observe the live allocation
                                // again; final Box retirement must still revoke it.
                                unsafe {
                                    register_subclass_for_test(base_pointer, pointer);
                                    register_subclass_for_test(pointer, child_pointer);
                                }
                            }
                        });
                        assert_eq!(callbacks, 5);
                        drop(view);
                    }
                    _ => {
                        let bridge = ObjectBridge::new();
                        let bits = MoltObject::from_int(31_006).bits();
                        let guard = (exit == "rollback")
                            .then(|| PublicationBuildGuard::enter(&bridge, bits).unwrap());
                        let entry = Box::new(BridgeEntry {
                            view,
                            bits,
                            unicode: None,
                            publication: if guard.is_some() {
                                PublicationState::Building {
                                    owner: std::thread::current().id(),
                                }
                            } else {
                                PublicationState::Ready
                            },
                            lifecycle: BridgeLifecycle::RuntimeOwned,
                        });
                        {
                            let (mut addresses, mut handles) =
                                bridge.lock_address_then_handle(address, bits);
                            addresses.from_py.insert(address, bits);
                            handles.to_py.insert(bits, entry);
                        }
                        if let Some(guard) = guard {
                            guard.inserted(pointer.cast());
                            drop(guard);
                        } else if exit == "deferred" {
                            assert!(bridge.retire_runtime_type_views(&[bits]));
                        } else {
                            assert_eq!(
                                bridge.release_pyobj(pointer.cast()),
                                PyObjRelease::ManagedViewRetired
                            );
                        }
                        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
                    }
                }
                assert_type_address_retired(address);
                crate::api::typeobj::unregister_type_address(base_pointer.addr());
                crate::api::typeobj::unregister_type_address(child_pointer.addr());
            }
        }
    }

    #[test]
    fn managed_type_extent_and_name_ownership_ignore_mutated_flags() {
        for allocated_heap in [false, true] {
            let mut prefix: PyTypeObject = unsafe { std::mem::zeroed() };
            prefix.tp_flags = if allocated_heap {
                crate::abi_types::Py_TPFLAGS_HEAPTYPE
            } else {
                0
            };
            let allocation = ManagedTypeAllocation::new(prefix);
            let pointer = allocation.get();
            let mut first = PyObject {
                ob_refcnt: 3,
                ob_type: std::ptr::null_mut(),
            };
            let mut second = PyObject {
                ob_refcnt: 4,
                ob_type: std::ptr::null_mut(),
            };
            if let Some(heap) = allocation.heap() {
                unsafe {
                    (*heap).ht_name = &raw mut first;
                    (*heap).ht_qualname = &raw mut second;
                }
            }
            unsafe {
                (*pointer).tp_flags ^= crate::abi_types::Py_TPFLAGS_HEAPTYPE;
            }
            assert_eq!(allocation.heap().is_some(), allocated_heap);
            let mut view = ManagedView::Type {
                object: allocation,
                _name: std::ffi::CString::new("storage.Type").unwrap(),
            };
            let mut released = Vec::new();
            view.release_owned_items_with(|edge, mirrored| {
                if !edge.is_null() {
                    released.push((edge, mirrored));
                }
            });
            if allocated_heap {
                assert_eq!(
                    released,
                    vec![(&raw mut first, true), (&raw mut second, true)]
                );
            } else {
                assert!(released.is_empty());
            }
            released.clear();
            view.release_owned_items_with(|edge, mirrored| {
                if !edge.is_null() {
                    released.push((edge, mirrored));
                }
            });
            assert!(released.is_empty(), "physical owners retire exactly once");
        }
    }

    #[test]
    fn type_publication_validates_exact_noncanonical_binding() {
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(31004).bits();
        let rebound = MoltObject::from_int(31005).bits();
        let mut shell: PyTypeObject = unsafe { std::mem::zeroed() };
        let pointer = &raw mut shell;
        unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(pointer.cast(), bits, false)
                .unwrap();
        }
        {
            let (address, handle) = bridge.lock_address_then_handle(pointer.addr(), bits);
            assert!(ObjectBridge::type_projection_matches(
                &address, &handle, bits, pointer, None, false
            ));
        }
        unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(pointer.cast(), rebound, false)
                .unwrap();
        }
        {
            let (address, handle) = bridge.lock_address_then_handle(pointer.addr(), bits);
            assert!(!ObjectBridge::type_projection_matches(
                &address, &handle, bits, pointer, None, false
            ));
        }
        unsafe {
            assert!(bridge.unbind_static_pyobj_from_runtime_handle(pointer.cast(), rebound));
        }
    }

    #[test]
    fn type_projection_query_includes_noncanonical_static_bindings() {
        let bridge = ObjectBridge::new();
        let bits = MoltObject::from_int(31003).bits();
        let mut shell = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let pointer = &raw mut shell;
        assert!(!bridge.type_has_projection(bits));
        unsafe {
            bridge
                .bind_static_pyobj_to_runtime_handle(pointer, bits, false)
                .unwrap();
        }
        assert!(bridge.type_has_projection(bits));
        unsafe {
            assert!(bridge.unbind_static_pyobj_from_runtime_handle(pointer, bits));
        }
        assert!(!bridge.type_has_projection(bits));
    }
}
