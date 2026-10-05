//! One namespace-to-physical-slot resolver for readiness and type mutation.
//! Names, ABI groups and typed dispatchers belong to SLOT_WRAPPER_DEFS.
use super::*;
use crate::abi_types::*;
use crate::api::{errors, refcount::OwnedPyObject, sequences, strings};

pub(crate) struct Mutation {
    root: OwnedPyObject,
    changed: OwnedPyObject,
    groups: Vec<(SlotWrapper, Vec<(usize, OwnedPyObject)>)>,
}

unsafe fn lookup_current(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
    key: *mut PyObject,
    owners: &mut Vec<OwnedPyObject>,
) -> Result<(*mut PyObject, *mut PyTypeObject), ()> {
    unsafe {
        let mro = (*tp).tp_mro;
        if mro.is_null() {
            errors::PyErr_SetString(
                (&raw mut PyExc_SystemError).cast(),
                c"native slot mutation requires a ready type MRO".as_ptr(),
            );
            return Err(());
        }
        owners.push(OwnedPyObject::from_borrowed(mro));
        let count = sequences::PyTuple_Size(mro);
        if count < 0 {
            return Err(());
        }
        for index in 0..count {
            let base = sequences::PyTuple_GetItem(mro, index).cast::<PyTypeObject>();
            if base.is_null() {
                return Err(());
            }
            let value = type_namespace_lookup_with_bridge(bridge, base, key);
            if descriptors::pending() {
                return Err(());
            }
            if !value.is_null() {
                owners.push(OwnedPyObject::from_borrowed(value));
                return Ok((value, base));
            }
        }
        Ok((ptr::null_mut(), ptr::null_mut()))
    }
}

/// A runtime wrapper retains its declared owner. Only that owner's actual
/// non-managed C shell supplies native slots; a Python facade's dispatch table
/// is never promoted to builtin authority. The pinned current MRO also admits
/// explicitly copied builtin descriptors without trusting the lookup position.
unsafe fn runtime_builtin_slot(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
    descriptor: *mut PyObject,
    row: &super::slot_wrappers::SlotWrapperDef,
) -> Option<*mut c_void> {
    unsafe {
        let descriptor = bridge.molt_handle_for_pyobj(descriptor)?;
        let name = std::ffi::CStr::from_ptr(row.base.name).to_bytes();
        let owner = (crate::hooks::hooks_or_stubs().builtin_slot_owner)(
            descriptor.bits(),
            name.as_ptr(),
            name.len(),
            row.slot == SlotWrapper::Direct(DirectSlot::New),
        );
        let crate::hooks::DecodedHandleResult::Ok(owner) = owner.decode() else {
            return None;
        };
        let mro = (*tp).tp_mro;
        for index in 0..sequences::PyTuple_Size(mro) {
            let base = sequences::PyTuple_GetItem(mro, index).cast::<PyTypeObject>();
            if base.is_null()
                || bridge.type_uses_runtime_slots(base)
                || bridge
                    .molt_handle_for_pyobj(base.cast())
                    .map(|value| value.bits())
                    != Some(owner)
            {
                continue;
            }
            let function = slot_wrapper_ptr(base, row.slot);
            if !function.is_null() && function != row.base.function {
                return Some(function);
            }
        }
        None
    }
}

/// Restrict only actual runtime native declarations. Ordinary Python classes
/// derive their protocols from special methods even when a native base exposes
/// only one of the same-name slots (e.g. dict/set subclasses have both lengths).
unsafe fn native_protocols(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
) -> Result<Option<u64>, ()> {
    if !bridge.type_uses_runtime_slots(tp) {
        return Ok(None);
    }
    let Some(class) = bridge.molt_handle_for_pyobj(tp.cast()) else {
        return Ok(None);
    };
    unsafe {
        match (crate::hooks::hooks_or_stubs().type_metadata)(
            class.bits(),
            crate::hooks::TypeMetadataField::NativeProtocolSlots,
        )
        .decode()
        {
            crate::hooks::DecodedHandleResult::Missing => Ok(None),
            crate::hooks::DecodedHandleResult::Error => {
                errors::check_native_status(-1, "native protocol declarations");
                Err(())
            }
            crate::hooks::DecodedHandleResult::Ok(bits) => {
                let value = molt_lang_obj_model::MoltObject::from_bits(bits).as_int();
                errors::with_preserved_error(|| (crate::hooks::hooks_or_stubs().dec_ref)(bits));
                match value {
                    Some(mask)
                        if mask >= 0
                            && (mask as u64) & !crate::hooks::NativeProtocolSlot::ALL_MASK == 0 =>
                    {
                        Ok(Some(mask as u64))
                    }
                    _ => {
                        errors::check_native_status(-1, "invalid native protocol declarations");
                        Err(())
                    }
                }
            }
        }
    }
}

fn protocol_slot(slot: SlotWrapper) -> Option<crate::hooks::NativeProtocolSlot> {
    use crate::hooks::NativeProtocolSlot as P;
    Some(match slot {
        SlotWrapper::Sequence(SequenceSlot::Length) => P::SequenceLength,
        SlotWrapper::Sequence(SequenceSlot::Concat) => P::SequenceConcat,
        SlotWrapper::Sequence(SequenceSlot::Repeat) => P::SequenceRepeat,
        SlotWrapper::Sequence(SequenceSlot::Item) => P::SequenceItem,
        SlotWrapper::Sequence(SequenceSlot::AssItem) => P::SequenceAssignItem,
        SlotWrapper::Sequence(SequenceSlot::Contains) => P::SequenceContains,
        SlotWrapper::Sequence(SequenceSlot::InPlaceConcat) => P::SequenceInPlaceConcat,
        SlotWrapper::Sequence(SequenceSlot::InPlaceRepeat) => P::SequenceInPlaceRepeat,
        SlotWrapper::Mapping(MappingSlot::Length) => P::MappingLength,
        SlotWrapper::Mapping(MappingSlot::Subscript) => P::MappingSubscript,
        SlotWrapper::Mapping(MappingSlot::AssSubscript) => P::MappingAssignSubscript,
        _ => return None,
    })
}

fn declarations_for_name(
    name: &[u8],
) -> impl Iterator<Item = &'static super::slot_wrappers::SlotWrapperDef> + '_ {
    SLOT_WRAPPER_DEFS.iter().filter(move |row| {
        // Every declaration name is a pinned, NUL-terminated static C string.
        unsafe { std::ffi::CStr::from_ptr(row.base.name).to_bytes() == name }
    })
}

pub(crate) fn affects_slots(name: &[u8]) -> bool {
    declarations_for_name(name).next().is_some()
}

/// A wrapper for one ABI must not manufacture another when exactly one of the
/// same-name declaring slots currently exists (CPython resolve_slotdups).
unsafe fn unique_existing_slot(tp: *mut PyTypeObject, name: *const c_char) -> Option<SlotWrapper> {
    unsafe {
        let bytes = std::ffi::CStr::from_ptr(name).to_bytes();
        let mut unique = None;
        for row in declarations_for_name(bytes) {
            if !slot_wrapper_ptr(tp, row.slot).is_null() {
                if unique.is_some_and(|slot| slot != row.slot) {
                    return None;
                }
                unique = Some(row.slot);
            }
        }
        unique
    }
}

/// Allocate canonical declaration names before dictionary commit. The actual
/// hierarchy and bindings must be read AFTER callback-capable key comparison.
pub(crate) unsafe fn prepare(tp: *mut PyTypeObject, name: *mut PyObject) -> Result<Mutation, ()> {
    unsafe {
        let bytes = strings::unicode_bytes(name).ok_or(())?.to_vec();
        let mut slots = Vec::new();
        for declaration in declarations_for_name(&bytes) {
            if !slots.contains(&declaration.slot) {
                slots.push(declaration.slot);
            }
        }
        prepare_slots(tp, name, slots)
    }
}

/// CPython fixup_slot_dispatchers and update_slot share update_one_slot. A
/// managed class already owns its Python declarations; physical readiness
/// resolves every canonical group without publishing native wrappers into it.
pub(crate) unsafe fn initialize(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
) -> c_int {
    unsafe {
        let mut slots = Vec::new();
        for row in SLOT_WRAPPER_DEFS {
            if !slots.contains(&row.slot) {
                slots.push(row.slot);
            }
        }
        let mutation = match prepare_slots(tp, ptr::null_mut(), slots) {
            Ok(mutation) => mutation,
            Err(()) => return -1,
        };
        let class = bridge
            .molt_handle_for_pyobj(tp.cast())
            .map(|value| value.bits());
        mutation.publish_bound(bridge, class)
    }
}

unsafe fn prepare_slots(
    tp: *mut PyTypeObject,
    name: *mut PyObject,
    slots: Vec<SlotWrapper>,
) -> Result<Mutation, ()> {
    unsafe {
        let root = OwnedPyObject::from_borrowed(tp.cast());
        let changed = OwnedPyObject::from_borrowed(name);
        let mut groups = Vec::new();
        for slot in slots {
            let mut declarations = Vec::new();
            for (index, row) in SLOT_WRAPPER_DEFS.iter().enumerate() {
                if row.slot == slot {
                    let key =
                        OwnedPyObject::from_owned(strings::PyUnicode_FromString(row.base.name));
                    if key.as_ptr().is_null() {
                        return Err(());
                    }
                    declarations.push((index, key));
                }
            }
            groups.push((slot, declarations));
        }
        Ok(Mutation {
            root,
            changed,
            groups,
        })
    }
}

impl Mutation {
    /// CPython update_slot runs after dictionary commit and cache invalidation.
    /// Retain all looked-up owners until the complete affected hierarchy has
    /// published. MRO lookup errors are suppressed per update_one_slot; errors
    /// checking subclass shadowing propagate, with the namespace committed.
    pub(crate) unsafe fn publish(&self) -> c_int {
        unsafe { self.publish_bound(&GLOBAL_BRIDGE, None) }
    }

    pub(crate) unsafe fn publish_for_runtime(&self, class: u64) -> c_int {
        unsafe { self.publish_bound(&GLOBAL_BRIDGE, Some(class)) }
    }

    fn root_is_bound(&self, bridge: &crate::bridge::ObjectBridge, class: Option<u64>) -> bool {
        class.is_none_or(|class| {
            bridge
                .molt_handle_for_pyobj(self.root.as_ptr())
                .map(|handle| handle.bits())
                == Some(class)
        })
    }

    unsafe fn publish_bound(
        &self,
        bridge: &crate::bridge::ObjectBridge,
        class: Option<u64>,
    ) -> c_int {
        unsafe {
            if self.groups.is_empty() {
                return 0;
            }
            let tp = self.root.as_ptr().cast::<PyTypeObject>();
            let name = self.changed.as_ptr();
            let mut owners = Vec::new();
            let Some(root) = type_identity(&mut TYPE_SUBCLASSES.lock(), tp) else {
                return -1;
            };
            let mut work = vec![root];
            let mut seen = HashSet::new();
            'hierarchy: while let Some(identity) = work.pop() {
                if !seen.insert(identity) {
                    continue;
                }
                let Some(current) = identity.live_type(&TYPE_SUBCLASSES.lock()) else {
                    continue;
                };
                let Some(owner) = OwnedPyObject::try_from_borrowed(current.cast()) else {
                    continue;
                };
                owners.push(owner);
                if !self.root_is_bound(bridge, class) {
                    return 0;
                }
                let current_binding = bridge.molt_handle_for_pyobj(current.cast());
                if current != tp {
                    let shadow = type_namespace_lookup_with_bridge(bridge, current, name);
                    if descriptors::pending() {
                        return -1;
                    }
                    if !self.root_is_bound(bridge, class) {
                        return 0;
                    }
                    if bridge.molt_handle_for_pyobj(current.cast()) != current_binding {
                        continue;
                    }
                    if !shadow.is_null() {
                        continue;
                    }
                }
                let protocols = match native_protocols(bridge, current) {
                    Ok(protocols) => protocols,
                    Err(()) => return -1,
                };
                if !self.root_is_bound(bridge, class) {
                    return 0;
                }
                if bridge.molt_handle_for_pyobj(current.cast()) != current_binding
                    || identity.live_type(&TYPE_SUBCLASSES.lock()) != Some(current)
                {
                    continue;
                }
                for (slot, declarations) in &self.groups {
                    let storage = slot_wrapper_storage(current, *slot);
                    if storage.is_null() {
                        continue;
                    }
                    let restricted = protocol_slot(*slot).is_some_and(|protocol| {
                        protocols.is_some_and(|mask| mask & protocol.bit() == 0)
                    });
                    let mut blocked_native_declaration = false;
                    let mut specific: *mut c_void = ptr::null_mut();
                    let mut generic: *mut c_void = ptr::null_mut();
                    let mut use_generic = false;
                    for (index, key) in declarations {
                        let row = &SLOT_WRAPPER_DEFS[*index];
                        let (descriptor, declaring_type) =
                            match lookup_current(bridge, current, key.as_ptr(), &mut owners) {
                                Ok(value) => value,
                                Err(()) => {
                                    errors::PyErr_Clear();
                                    (ptr::null_mut(), ptr::null_mut())
                                }
                            };
                        if !self.root_is_bound(bridge, class) {
                            return 0;
                        }
                        if bridge.molt_handle_for_pyobj(current.cast()) != current_binding
                            || identity.live_type(&TYPE_SUBCLASSES.lock()) != Some(current)
                        {
                            continue 'hierarchy;
                        }
                        if descriptor.is_null() {
                            if *slot == SlotWrapper::Direct(DirectSlot::IterNext) {
                                specific = native_slot_dispatch::next_not_implemented as *const ()
                                    as *mut c_void;
                            }
                            continue;
                        }
                        // Exact native policy constrains this class's own
                        // declarations. A native subtype can inherit only a
                        // slot actually present on the namespace that supplied
                        // the resolved method; MRO mask unions would resurrect
                        // slots disabled by a nearer native declaration.
                        if restricted
                            && (declaring_type == current
                                || slot_wrapper_ptr(declaring_type, *slot).is_null())
                        {
                            // One slot may have several names (__mul__/__rmul__).
                            // An excluded declaration blocks the entire slot,
                            // even when another name reaches a more distant base.
                            blocked_native_declaration = true;
                            continue;
                        }
                        if descriptor == &raw mut Py_None
                            && *slot == SlotWrapper::Direct(DirectSlot::Hash)
                        {
                            specific = PyObject_HashNotImplemented as *const () as *mut c_void;
                        } else if *slot == SlotWrapper::Direct(DirectSlot::New)
                            && object_construction::is_new_wrapper(descriptor)
                        {
                            // CPython update_one_slot: the C __new__ wrapper
                            // preserves the already selected native constructor.
                            specific = slot_wrapper_ptr(current, *slot);
                        } else if let Some(function) =
                            runtime_builtin_slot(bridge, current, descriptor, row)
                        {
                            generic = row.base.function;
                            if specific.is_null() || specific == function {
                                specific = function;
                            } else {
                                use_generic = true;
                            }
                        } else if (*descriptor).ob_type == &raw mut PyWrapperDescr_Type {
                            let wrapper = descriptor.cast::<PyWrapperDescrObject>();
                            let base = (*wrapper).d_base;
                            let same_name = !base.is_null()
                                && std::ffi::CStr::from_ptr((*base).name).to_bytes()
                                    == std::ffi::CStr::from_ptr(row.base.name).to_bytes();
                            if same_name {
                                if unique_existing_slot(current, row.base.name)
                                    .is_none_or(|unique| unique == *slot)
                                {
                                    generic = row.base.function;
                                }
                                let compatible = (*base).wrapper.map(|f| f as *const ())
                                    == row.base.wrapper.map(|f| f as *const ())
                                    && PyType_IsSubtype(current, (*wrapper).d_common.d_type) != 0;
                                if compatible
                                    && (specific.is_null() || specific == (*wrapper).d_wrapped)
                                {
                                    specific = (*wrapper).d_wrapped;
                                } else {
                                    use_generic = true;
                                }
                            } else {
                                use_generic = true;
                                generic = row.base.function;
                            }
                        } else {
                            use_generic = true;
                            generic = row.base.function;
                        }
                    }
                    let target = if blocked_native_declaration {
                        ptr::null_mut()
                    } else if !specific.is_null() && !use_generic {
                        specific
                    } else {
                        generic
                    };
                    // Lookup can invoke equality/descriptors and replace a
                    // protocol table. Resolve the live destination only after
                    // identity validation, while the exact owner is retained.
                    let storage = slot_wrapper_storage(current, *slot);
                    if storage.is_null() {
                        continue;
                    }
                    storage.write(target);
                    if *slot == SlotWrapper::Direct(DirectSlot::Call)
                        && target == native_slot_dispatch::dispatcher(*slot)
                    {
                        (*current).tp_flags &= !Py_TPFLAGS_HAVE_VECTORCALL;
                    }
                }
                if !self.root_is_bound(bridge, class) {
                    return 0;
                }
                if bridge.molt_handle_for_pyobj(current.cast()) != current_binding {
                    continue;
                }
                if name.is_null() {
                    continue;
                }
                let mut registry = TYPE_SUBCLASSES.lock();
                if let Some(children) = registry.subclasses.get_mut(&identity) {
                    children.compact();
                    work.extend(children.order.iter().rev().copied());
                }
            }
            0
        }
    }
}
